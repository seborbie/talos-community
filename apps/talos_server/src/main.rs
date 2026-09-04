use std::{
    collections::HashMap,
    env,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        Path, Query, State,
    },
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::{delete, get, post},
    Json, Router,
};
use base64::engine::general_purpose::{
    STANDARD as BASE64_STANDARD, URL_SAFE_NO_PAD as BASE64_URL_SAFE_NO_PAD,
};
use base64::Engine as _;
use chrono::{DateTime, Utc};
use futures_util::{SinkExt, StreamExt};
use rand::{rngs::OsRng, RngCore};
use rcgen::generate_simple_self_signed;
use reqwest::Client;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::Value;
use talos_protocol::{
    AgentFeatureCapabilities, AgentHello, AgentPlatform, ChatSessionCapabilitiesHttpResponse,
    FileTransferSessionCapabilitiesHttpResponse, FullSnapshotUpdate, IncomingEnvelope,
    LinuxShellCredentialPayload, LinuxShellCredentialStoredPayload, LocalAddr,
    MacosUpdateAccountStatusPayload, OutgoingEnvelope, PunchStartPayload, QuicReflexPayload,
    ReflexAddress, RegistrySessionCapabilitiesHttpResponse, RelayPreparePayload,
    RemoteDesktopCapabilities, RemoteDesktopUnavailablePayload, RequestFullSnapshotPayload,
    SessionCapabilitiesHttpResponse, SessionCapabilitiesRequest, SessionCapabilitiesResponse,
    SessionTransportMode, ShellCommandPayload, ShellOfferPayload, ShellOutputPayload,
    ShellSessionCapabilitiesHttpResponse, TelemetryEventsUpdate, TunnelPreparePayload,
    FILE_TRANSFER_DEFAULT_CHUNK_BYTES, FILE_TRANSFER_DEFAULT_ZIP_THRESHOLD_BYTES,
    FILE_TRANSFER_DEFAULT_ZIP_THRESHOLD_FILES, REMOTE_DESKTOP_PROFILE_EXPERIMENTAL,
    REMOTE_DESKTOP_PROFILE_LEGACY, REMOTE_DESKTOP_PROFILE_MODERN_CPU,
    REMOTE_DESKTOP_PROFILE_MODERN_GPU, REMOTE_DESKTOP_PROFILE_SCREENSHOT_ONLY,
};
use tokio::{
    net::TcpListener,
    sync::{mpsc, oneshot, RwLock},
    time::timeout,
};
use tower_http::{
    cors::{Any, CorsLayer},
    trace::TraceLayer,
};
use tracing::{error, info, warn};
use uuid::Uuid;

mod agent_directory;
mod config;
mod remediation;

use agent_directory::{AgentConnection, AgentDirectory, AgentRegistration};
use config::{load_config, Config};
use remediation::{
    notification_targets, status_request_body, ApiRemediationJobStatusResponse,
    ApiRemediationJobsClaimResponse, RemediationCommandJob, RemediationCommandsEnqueueRequest,
    RemediationCommandsEnqueueResponse, RemediationJobUpdatePayload,
    RemediationJobsAvailablePayload, RemediationJobsPollPayload, RemediationJobsResponsePayload,
};

#[derive(Clone)]
struct AppState {
    config: Arc<Config>,
    api_client: Client,
    agents: AgentDirectory,
    shell_sessions: Arc<RwLock<HashMap<String, PendingShellSession>>>,
    active_shell_sessions: Arc<RwLock<HashMap<String, ActiveShellSession>>>,
    detail_requests: Arc<RwLock<HashMap<String, PendingDetailRequest>>>,
    shell_commands: Arc<RwLock<HashMap<String, PendingShellCommand>>>,
    rdp_sessions_requests: Arc<RwLock<HashMap<String, PendingRdpSessionsRequest>>>,
    capability_requests: Arc<RwLock<HashMap<String, PendingCapabilityRequest>>>,
    quic_reflex_requests: Arc<RwLock<HashMap<String, PendingQuicReflex>>>,
    remote_desktop_sessions: Arc<RwLock<HashMap<String, RemoteDesktopSession>>>,
    file_transfer_sessions: Arc<RwLock<HashMap<String, FileTransferSession>>>,
    remote_registry_sessions: Arc<RwLock<HashMap<String, RemoteRegistrySession>>>,
    chat_sessions: Arc<RwLock<HashMap<String, ChatSession>>>,
    last_snapshot_request: Arc<RwLock<HashMap<String, Instant>>>,
}

const PENDING_REQUEST_TTL: Duration = Duration::from_secs(60);
const UNATTACHED_SESSION_TTL: Duration = Duration::from_secs(15 * 60);
const SESSION_REAPER_INTERVAL: Duration = Duration::from_secs(30);
const VIEWER_HEARTBEAT_TTL: Duration = Duration::from_secs(6);

struct PendingShellSession {
    response_tx: oneshot::Sender<Result<ShellOfferPayload, String>>,
    created_at: Instant,
}

#[derive(Clone)]
struct ViewerIdentity {
    user_id: String,
    user_email: Option<String>,
}

#[derive(Clone)]
struct ActiveShellSession {
    token: String,
    agent_id: String,
    viewer_user_id: Option<String>,
    viewer_user_email: Option<String>,
    transports: Vec<String>,
    agent_reflex: Option<ReflexAddress>,
    viewer_reflex: Option<ReflexAddress>,
    agent_host: Option<String>,
    agent_local_addrs: Vec<LocalAddr>,
    platform: AgentPlatform,
    features: AgentFeatureCapabilities,
    psk_cert_pem: Option<String>,
    e2e_key: Vec<u8>,
    relay_url: Option<String>,
    created_at: Instant,
    attached_at: Option<Instant>,
    viewer_connected_at: Option<Instant>,
    viewer_last_heartbeat_at: Option<Instant>,
}

struct PendingDetailRequest {
    response_tx: oneshot::Sender<Value>,
    created_at: Instant,
}

struct PendingShellCommand {
    response_tx: oneshot::Sender<ShellOutputPayload>,
    created_at: Instant,
}

struct PendingRdpSessionsRequest {
    response_tx: oneshot::Sender<Vec<RdpSessionInfo>>,
    created_at: Instant,
}

struct PendingCapabilityRequest {
    response_tx: oneshot::Sender<RemoteDesktopCapabilities>,
    created_at: Instant,
}

enum QuicReflexResult {
    Success(ReflexAddress),
    DisplayUnavailable {
        reason: String,
        message: Option<String>,
    },
}

struct PendingQuicReflex {
    response_tx: oneshot::Sender<QuicReflexResult>,
    created_at: Instant,
}

#[derive(Clone)]
struct RemoteDesktopSession {
    token: String,
    mode: SessionTransportMode,
    hide_cursor: bool,
    viewer_user_id: Option<String>,
    viewer_user_email: Option<String>,
    capabilities: RemoteDesktopCapabilities,
    /// None when STUN/UDP is blocked; relay still works.
    agent_reflex: Option<ReflexAddress>,
    viewer_reflex: Option<ReflexAddress>,
    agent_host: Option<String>,
    agent_local_addrs: Vec<LocalAddr>,
    psk_cert_pem: String,
    e2e_key: Vec<u8>,
    relay_url: Option<String>,
    agent_id: String,
    agent_hostname: Option<String>,
    agent_os: Option<String>,
    agent_version: Option<String>,
    created_at: Instant,
    attached_at: Option<Instant>,
    viewer_connected_at: Option<Instant>,
    viewer_last_heartbeat_at: Option<Instant>,
}

#[derive(Clone)]
struct FileTransferSession {
    token: String,
    viewer_user_id: Option<String>,
    viewer_user_email: Option<String>,
    transports: Vec<String>,
    /// None when STUN/UDP is blocked; relay still works.
    agent_reflex: Option<ReflexAddress>,
    viewer_reflex: Option<ReflexAddress>,
    agent_host: Option<String>,
    agent_local_addrs: Vec<LocalAddr>,
    platform: AgentPlatform,
    features: AgentFeatureCapabilities,
    psk_cert_pem: String,
    e2e_key: Vec<u8>,
    relay_url: Option<String>,
    agent_id: String,
    created_at: Instant,
    attached_at: Option<Instant>,
    viewer_connected_at: Option<Instant>,
    viewer_last_heartbeat_at: Option<Instant>,
}

#[derive(Clone)]
struct RemoteRegistrySession {
    token: String,
    viewer_user_id: Option<String>,
    viewer_user_email: Option<String>,
    transports: Vec<String>,
    agent_reflex: Option<ReflexAddress>,
    viewer_reflex: Option<ReflexAddress>,
    agent_host: Option<String>,
    agent_local_addrs: Vec<LocalAddr>,
    platform: AgentPlatform,
    features: AgentFeatureCapabilities,
    psk_cert_pem: String,
    e2e_key: Vec<u8>,
    relay_url: Option<String>,
    agent_id: String,
    created_at: Instant,
    attached_at: Option<Instant>,
    viewer_connected_at: Option<Instant>,
    viewer_last_heartbeat_at: Option<Instant>,
}

#[derive(Clone)]
struct ChatSession {
    token: String,
    viewer_user_id: Option<String>,
    viewer_user_email: Option<String>,
    parent_desktop_session_id: Option<String>,
    transports: Vec<String>,
    agent_reflex: Option<ReflexAddress>,
    viewer_reflex: Option<ReflexAddress>,
    agent_host: Option<String>,
    agent_local_addrs: Vec<LocalAddr>,
    platform: AgentPlatform,
    features: AgentFeatureCapabilities,
    psk_cert_pem: String,
    e2e_key: Vec<u8>,
    relay_url: Option<String>,
    agent_id: String,
    created_at: Instant,
    attached_at: Option<Instant>,
    viewer_connected_at: Option<Instant>,
    viewer_last_heartbeat_at: Option<Instant>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ShellConnectQuery {
    run_as: Option<ShellRunAs>,
    target_session_id: Option<u32>,
    session: Option<String>,
    token: Option<String>,
}

#[derive(Deserialize)]
struct TokenQuery {
    token: Option<String>,
}

#[derive(Deserialize)]
struct ExecuteScriptRequest {
    script: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PatchJobsPollPayload {
    request_id: String,
    limit: Option<u32>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PatchJobUpdatePayload {
    job_id: String,
    status: String,
    step_index: Option<i32>,
    evidence: Option<Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PatchJobProgressPayload {
    organization_id: String,
    agent_id: String,
    job_id: String,
    command_id: String,
    status: String,
    phase: String,
    #[serde(flatten)]
    extra: serde_json::Map<String, Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PatchStateCheckinPayload {
    #[serde(default)]
    request_id: Option<String>,
    #[serde(default)]
    observed_at: Option<String>,
    #[serde(default)]
    state: Value,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PatchActionResultPayload {
    operation_id: String,
    action: String,
    status: String,
    #[serde(default)]
    update_keys: Vec<String>,
    #[serde(default)]
    evidence: Value,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ApiPatchActionPlanResponse {
    plan: Value,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PatchActionPlanPayload {
    request_id: Option<String>,
    plan: Value,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ApiPatchActionResultResponse {
    accepted: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PatchJobsNotifyRequest {
    agent_ids: Option<Vec<String>>,
    reason: Option<String>,
    requested_by: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct PatchRemediationStep {
    id: String,
    step_index: i32,
    command: String,
    status: String,
    evidence: Option<Value>,
    started_at: Option<String>,
    finished_at: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct PatchRemediationJob {
    id: String,
    organization_id: String,
    agent_id: String,
    intent_id: String,
    status: String,
    dedupe_key: Option<String>,
    metadata: Value,
    requested_at: String,
    started_at: Option<String>,
    finished_at: Option<String>,
    steps: Vec<PatchRemediationStep>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ApiPatchJobsClaimResponse {
    jobs: Vec<PatchRemediationJob>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ApiPatchJobStatusResponse {
    updated: bool,
    status: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PatchJobsResponsePayload {
    request_id: String,
    jobs: Vec<PatchRemediationJob>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct PatchJobsAvailablePayload {
    reason: String,
    requested_by: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PatchJobsNotifyResponse {
    connected_agents: usize,
    notified_agents: usize,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FeatureUpgradePreflightNotifyRequest {
    agent_ids: Option<Vec<String>>,
    reason: Option<String>,
    requested_by: Option<String>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct FeatureUpgradePreflightJobsAvailablePayload {
    reason: String,
    requested_by: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FeatureUpgradePreflightJobsPollPayload {
    request_id: String,
    limit: Option<u32>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ApiFeatureUpgradePreflightJobsClaimResponse {
    jobs: Vec<Value>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FeatureUpgradePreflightJobsResponsePayload {
    request_id: String,
    jobs: Vec<Value>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct FeatureUpgradePreflightProgressPayload {
    operation_id: String,
    run_id: String,
    organization_id: String,
    agent_id: String,
    status: String,
    phase: String,
    #[serde(default)]
    checks: Vec<Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ApiFeatureUpgradePreflightProgressResponse {
    accepted: bool,
    updated: usize,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FeatureUpgradeStageIsoNotifyRequest {
    agent_ids: Option<Vec<String>>,
    reason: Option<String>,
    requested_by: Option<String>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct FeatureUpgradeStageIsoJobsAvailablePayload {
    reason: String,
    requested_by: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FeatureUpgradeStageIsoJobsPollPayload {
    request_id: String,
    limit: Option<u32>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ApiFeatureUpgradeStageIsoJobsClaimResponse {
    jobs: Vec<Value>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FeatureUpgradeStageIsoJobsResponsePayload {
    request_id: String,
    jobs: Vec<Value>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct FeatureUpgradeStageIsoProgressPayload {
    operation_id: String,
    run_id: String,
    organization_id: String,
    agent_id: String,
    status: String,
    phase: String,
    #[serde(flatten)]
    extra: serde_json::Map<String, Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ApiFeatureUpgradeStageIsoProgressResponse {
    accepted: bool,
    updated: usize,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FeatureUpgradeStartNotifyRequest {
    agent_ids: Option<Vec<String>>,
    reason: Option<String>,
    requested_by: Option<String>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct FeatureUpgradeStartJobsAvailablePayload {
    reason: String,
    requested_by: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FeatureUpgradeStartJobsPollPayload {
    request_id: String,
    limit: Option<u32>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ApiFeatureUpgradeStartJobsClaimResponse {
    jobs: Vec<Value>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FeatureUpgradeStartJobsResponsePayload {
    request_id: String,
    jobs: Vec<Value>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct FeatureUpgradeStartProgressPayload {
    operation_id: String,
    run_id: String,
    organization_id: String,
    agent_id: String,
    status: String,
    phase: String,
    #[serde(flatten)]
    extra: serde_json::Map<String, Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ApiFeatureUpgradeStartProgressResponse {
    accepted: bool,
    updated: usize,
}

#[derive(Deserialize)]
struct SessionCapabilitiesQuery {
    token: String,
}

#[derive(Deserialize)]
struct SessionDeviceInfoQuery {
    token: String,
    refresh: Option<String>,
}

#[derive(Deserialize)]
struct ViewerReflexQuery {
    token: String,
}

#[derive(Deserialize)]
struct ViewerReflexRequest {
    ip: String,
    port: u16,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AiRunnerSessionCleanupRequest {
    session_id: String,
    kind: String,
    agent_id: String,
}

struct UserContext {
    user_id: String,
    organization_id: String,
    role: String,
    email: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ApiUserContext {
    user_id: String,
    organization_id: String,
    role: String,
    email: Option<String>,
}

#[derive(Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum ViewerSessionKind {
    RemoteDesktop,
    Shell,
    FileTransfer,
    RemoteRegistry,
    Chat,
}

#[derive(Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum ViewerSessionLaunchState {
    Pending,
    Connected,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ViewerSessionStatusResponse {
    session_id: String,
    kind: ViewerSessionKind,
    agent_id: String,
    user_id: Option<String>,
    user_email: Option<String>,
    state: ViewerSessionLaunchState,
    connected: bool,
    attached: bool,
    connected_at: Option<DateTime<Utc>>,
    last_heartbeat_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ViewerConnectionSummary {
    session_id: String,
    kind: ViewerSessionKind,
    agent_id: String,
    user_id: Option<String>,
    user_email: Option<String>,
    connected_at: Option<DateTime<Utc>>,
    last_heartbeat_at: Option<DateTime<Utc>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ViewerConnectionsQuery {
    agent_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ApiDeviceScope {
    organization_id: String,
    customer_id: Option<String>,
    site_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ApiEnrollmentResponse {
    enrolled: bool,
    organization_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ApiValidationResponse {
    allowed: bool,
    reason: Option<String>,
    matched_policy_id: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct BulkUpdateCustomerResponse {
    updated: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DeleteDevicesResponse {
    deleted: u64,
}

#[derive(Serialize)]
struct ScriptResponse {
    output: String,
    exit_code: Option<i32>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DeviceSummary {
    agent_id: String,
    hostname: String,
    os: String,
    ip: String,
    version: Option<String>,
    last_seen: DateTime<Utc>,
    last_inventory: Option<Value>,
    device_details: Option<Value>,
    customer_id: Option<String>,
    customer_name: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ApiDeviceSummary {
    agent_id: String,
    hostname: String,
    os: String,
    ip: String,
    version: Option<String>,
    last_seen: DateTime<Utc>,
    last_inventory: Option<Value>,
    device_details: Option<Value>,
    customer_id: Option<String>,
    customer_name: Option<String>,
}

#[derive(Deserialize)]
struct ApiAccepted {
    accepted: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
struct ApiSnapshotRequestStatusResponse {
    request_id: String,
    status: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct InternalAiRunnerConnectRequest {
    organization_id: String,
    job_id: Option<String>,
    api_base_url: Option<String>,
    desktop_mode: Option<String>,
    #[serde(default)]
    display_profile_preference: Vec<String>,
    hide_cursor: Option<bool>,
    run_as: Option<ShellRunAs>,
    target_session_id: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AiRunnerDesktopMode {
    Interactive,
    ScreenshotOnly,
}

impl AiRunnerDesktopMode {
    fn from_request(value: Option<&str>) -> Self {
        match value.map(str::trim).filter(|value| !value.is_empty()) {
            Some(value) if value.eq_ignore_ascii_case("interactive") => Self::Interactive,
            _ => Self::ScreenshotOnly,
        }
    }
}

impl From<ApiDeviceSummary> for DeviceSummary {
    fn from(value: ApiDeviceSummary) -> Self {
        Self {
            agent_id: value.agent_id,
            hostname: value.hostname,
            os: value.os,
            ip: value.ip,
            version: value.version,
            last_seen: value.last_seen,
            last_inventory: value.last_inventory,
            device_details: value.device_details,
            customer_id: value.customer_id,
            customer_name: value.customer_name,
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ConnectResponse {
    url: String,
    session_id: String,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LinuxShellCredentialResponse {
    agent_id: String,
    username: String,
    password: String,
    credential_id: Option<String>,
    version: Option<i32>,
    updated_at: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ApiCredentialStoreResponse {
    accepted: bool,
    credential_id: String,
    stored_at: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SessionDeviceInfoHttpResponse {
    device: DeviceSummary,
    refreshed: bool,
    refresh_error: Option<String>,
}

#[derive(Deserialize)]
struct InventoryUpdate {
    agent_id: String,
    hostname: String,
    os: String,
    ip: String,
    version: Option<String>,
    inventory: Value,
}

#[derive(Serialize)]
struct FetchDetailsPayload {
    request_id: String,
}

#[derive(Deserialize)]
struct DeviceDetailsPayload {
    request_id: String,
    details: Value,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ShellStartPayload {
    session_id: String,
    token: String,
    run_as: ShellRunAs,
    #[serde(skip_serializing_if = "Option::is_none")]
    target_session_id: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    relay_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    e2e_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    psk_cert_pem: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    psk_key_pem: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum ShellRunAs {
    User,
    System,
}

fn platform_from_os(os: Option<&str>) -> AgentPlatform {
    let normalized = os.unwrap_or_default().to_ascii_lowercase();
    if normalized.contains("windows") {
        AgentPlatform::Windows
    } else if normalized.contains("linux")
        || normalized.contains("ubuntu")
        || normalized.contains("debian")
        || normalized.contains("fedora")
        || normalized.contains("rhel")
        || normalized.contains("centos")
        || normalized.contains("rocky")
        || normalized.contains("alma")
        || normalized.contains("suse")
        || normalized.contains("arch")
    {
        AgentPlatform::Linux
    } else if normalized.contains("macos")
        || normalized.contains("mac os")
        || normalized.contains("darwin")
    {
        AgentPlatform::Macos
    } else {
        AgentPlatform::Unknown
    }
}

fn normalize_agent_platform(platform: AgentPlatform, os: Option<&str>) -> AgentPlatform {
    if platform != AgentPlatform::Unknown {
        platform
    } else {
        platform_from_os(os)
    }
}

fn features_for_platform(platform: AgentPlatform) -> AgentFeatureCapabilities {
    AgentFeatureCapabilities::for_platform(platform)
}

fn features_for_agent(agent: &AgentConnection) -> AgentFeatureCapabilities {
    let mut features = if agent.features == AgentFeatureCapabilities::default()
        && agent.platform != AgentPlatform::Windows
    {
        features_for_platform(agent.platform)
    } else {
        agent.features.clone()
    };
    if agent.platform == AgentPlatform::Macos && !agent.is_admin {
        features.system_shell = false;
    }
    features
}

fn app_error_for_unsupported(feature: &str, platform: AgentPlatform) -> AppError {
    AppError::bad_request(&format!("{feature} unsupported_platform ({platform:?})"))
}

fn system_shell_requires_elevation_message(platform: AgentPlatform) -> &'static str {
    match platform {
        AgentPlatform::Macos => {
            "macOS system shell unavailable; install and run the Talos worker as a root LaunchDaemon"
        }
        _ => "system shell unavailable (agent not elevated)",
    }
}

fn user_shell_unsupported_message(platform: AgentPlatform) -> &'static str {
    match platform {
        AgentPlatform::Macos => {
            "user shell unavailable on macOS; macOS shell sessions run as system/root"
        }
        AgentPlatform::Linux => {
            "user shell unavailable on Linux; Linux shell sessions use the configured shell user"
        }
        _ => "user shell unavailable on this platform",
    }
}

#[derive(Deserialize)]
struct ShellErrorPayload {
    session_id: String,
    error: String,
}

#[derive(Serialize)]
struct RdpSessionsRequestPayload {
    request_id: String,
}

#[derive(Deserialize)]
struct RdpSessionsResponsePayload {
    request_id: String,
    sessions: Vec<RdpSessionInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RdpSessionInfo {
    logical_session_id: u32,
    native_session_id: u32,
    kind: String,
    win_station: String,
    user_name: String,
    state: String,
}

#[derive(Serialize)]
struct RdpSessionsHttpResponse {
    sessions: Vec<RdpSessionInfo>,
}

#[tokio::main]
async fn main() -> Result<()> {
    load_dotenv();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let config = load_config()?;
    let api_client = Client::new();

    let state = AppState {
        config: Arc::new(config),
        api_client,
        agents: AgentDirectory::default(),
        shell_sessions: Arc::new(RwLock::new(HashMap::new())),
        active_shell_sessions: Arc::new(RwLock::new(HashMap::new())),
        detail_requests: Arc::new(RwLock::new(HashMap::new())),
        shell_commands: Arc::new(RwLock::new(HashMap::new())),
        rdp_sessions_requests: Arc::new(RwLock::new(HashMap::new())),
        capability_requests: Arc::new(RwLock::new(HashMap::new())),
        quic_reflex_requests: Arc::new(RwLock::new(HashMap::new())),
        remote_desktop_sessions: Arc::new(RwLock::new(HashMap::new())),
        file_transfer_sessions: Arc::new(RwLock::new(HashMap::new())),
        remote_registry_sessions: Arc::new(RwLock::new(HashMap::new())),
        chat_sessions: Arc::new(RwLock::new(HashMap::new())),
        last_snapshot_request: Arc::new(RwLock::new(HashMap::new())),
    };
    spawn_session_reaper(state.clone());

    let app = Router::new()
        .route("/api/rmm/health", get(health))
        .route("/api/rmm/devices", get(list_devices))
        .route(
            "/api/rmm/devices/bulk-update-customer",
            post(bulk_update_customer),
        )
        .route("/api/rmm/devices/bulk-delete", post(bulk_delete_devices))
        .route("/api/rmm/devices/:agent_id", get(get_device))
        .route("/api/rmm/devices/:agent_id", delete(delete_device))
        .route(
            "/api/rmm/devices/:agent_id/fetch-details",
            post(fetch_device_details),
        )
        .route(
            "/api/rmm/devices/:agent_id/request-snapshot",
            post(request_snapshot),
        )
        .route(
            "/api/rmm/devices/:agent_id/execute-script",
            post(execute_script),
        )
        .route(
            "/api/rmm/devices/:agent_id/patch-jobs/notify",
            post(notify_patch_jobs_for_device),
        )
        .route(
            "/api/rmm/internal/patch-jobs/notify",
            post(notify_patch_jobs_internal),
        )
        .route(
            "/api/rmm/internal/feature-upgrades/preflight/notify",
            post(notify_feature_upgrade_preflight_internal),
        )
        .route(
            "/api/rmm/internal/feature-upgrades/stage-iso/notify",
            post(notify_feature_upgrade_stage_iso_internal),
        )
        .route(
            "/api/rmm/internal/feature-upgrades/start/notify",
            post(notify_feature_upgrade_start_internal),
        )
        .route(
            "/api/rmm/internal/remediation/commands/enqueue",
            post(enqueue_remediation_commands_internal),
        )
        .route(
            "/api/rmm/internal/ai-runner/devices/:agent_id/connect",
            post(connect_ai_runner_desktop_session_internal),
        )
        .route(
            "/api/rmm/internal/ai-runner/devices/:agent_id/chat-approval",
            post(connect_ai_runner_chat_approval_internal),
        )
        .route(
            "/api/rmm/internal/ai-runner/devices/:agent_id/connect-shell",
            post(connect_ai_runner_shell_session_internal),
        )
        .route(
            "/api/rmm/internal/ai-runner/sessions/cleanup",
            post(cleanup_ai_runner_session_internal),
        )
        .route(
            "/api/rmm/devices/:agent_id/rdp-sessions",
            get(list_rdp_sessions),
        )
        .route("/api/rmm/devices/:agent_id/connect", post(connect_device))
        .route(
            "/api/rmm/devices/:agent_id/connect-file-transfer",
            post(connect_file_transfer),
        )
        .route(
            "/api/rmm/devices/:agent_id/connect-shell",
            post(connect_shell),
        )
        .route(
            "/api/rmm/devices/:agent_id/connect-registry",
            post(connect_registry),
        )
        .route("/api/rmm/viewer-connections", get(list_viewer_connections))
        .route(
            "/api/rmm/viewer-session/:session_id/status",
            get(get_viewer_session_status),
        )
        .route(
            "/api/rmm/shell/session/:session_id/open-desktop",
            post(open_desktop_from_shell),
        )
        .route(
            "/api/rmm/shell/session/:session_id/open-file-transfer",
            post(open_file_transfer_from_shell),
        )
        .route(
            "/api/rmm/shell/session/:session_id/open-registry",
            post(open_registry_from_shell),
        )
        .route(
            "/api/rmm/shell/session/:session_id/end",
            post(end_shell_session),
        )
        .route(
            "/api/rmm/shell/session/:session_id/capabilities",
            get(get_shell_capabilities),
        )
        .route(
            "/api/rmm/shell/session/:session_id/viewer-reflex",
            post(shell_viewer_reflex),
        )
        .route(
            "/api/rmm/shell/session/:session_id/viewer-connected",
            post(shell_viewer_connected),
        )
        .route(
            "/api/rmm/shell/session/:session_id/viewer-heartbeat",
            post(shell_viewer_heartbeat),
        )
        .route(
            "/api/rmm/shell/session/:session_id/request-relay",
            post(request_shell_relay),
        )
        .route(
            "/api/rmm/shell/session/:session_id/linux-shell-credential",
            get(get_linux_shell_credential_for_shell_session),
        )
        .route(
            "/api/rmm/session/:session_id/capabilities",
            get(get_session_capabilities),
        )
        .route(
            "/api/rmm/session/:session_id/open-file-transfer",
            post(open_file_transfer_from_session),
        )
        .route(
            "/api/rmm/session/:session_id/open-registry",
            post(open_registry_from_session),
        )
        .route(
            "/api/rmm/session/:session_id/open-chat",
            post(open_chat_from_session),
        )
        .route(
            "/api/rmm/session/:session_id/device-info",
            get(get_session_device_info),
        )
        .route(
            "/api/rmm/session/:session_id/request-relay",
            post(request_relay),
        )
        .route(
            "/api/rmm/session/:session_id/viewer-reflex",
            post(viewer_reflex),
        )
        .route(
            "/api/rmm/session/:session_id/viewer-connected",
            post(viewer_connected),
        )
        .route(
            "/api/rmm/session/:session_id/viewer-heartbeat",
            post(viewer_heartbeat),
        )
        .route(
            "/api/rmm/session/:session_id/end",
            post(end_remote_desktop_session),
        )
        .route(
            "/api/rmm/registry/session/:session_id/capabilities",
            get(get_registry_capabilities),
        )
        .route(
            "/api/rmm/registry/session/:session_id/request-relay",
            post(request_registry_relay),
        )
        .route(
            "/api/rmm/registry/session/:session_id/viewer-reflex",
            post(registry_viewer_reflex),
        )
        .route(
            "/api/rmm/registry/session/:session_id/viewer-connected",
            post(registry_viewer_connected),
        )
        .route(
            "/api/rmm/registry/session/:session_id/viewer-heartbeat",
            post(registry_viewer_heartbeat),
        )
        .route(
            "/api/rmm/registry/session/:session_id/end",
            post(end_remote_registry_session),
        )
        .route(
            "/api/rmm/file-transfer/session/:session_id/capabilities",
            get(get_file_transfer_capabilities),
        )
        .route(
            "/api/rmm/file-transfer/session/:session_id/request-relay",
            post(request_file_transfer_relay),
        )
        .route(
            "/api/rmm/file-transfer/session/:session_id/viewer-reflex",
            post(file_transfer_viewer_reflex),
        )
        .route(
            "/api/rmm/file-transfer/session/:session_id/viewer-connected",
            post(file_transfer_viewer_connected),
        )
        .route(
            "/api/rmm/file-transfer/session/:session_id/viewer-heartbeat",
            post(file_transfer_viewer_heartbeat),
        )
        .route(
            "/api/rmm/file-transfer/session/:session_id/end",
            post(end_file_transfer_session),
        )
        .route(
            "/api/rmm/chat/session/:session_id/capabilities",
            get(get_chat_capabilities),
        )
        .route(
            "/api/rmm/chat/session/:session_id/request-relay",
            post(request_chat_relay),
        )
        .route(
            "/api/rmm/chat/session/:session_id/viewer-reflex",
            post(chat_viewer_reflex),
        )
        .route(
            "/api/rmm/chat/session/:session_id/viewer-connected",
            post(chat_viewer_connected),
        )
        .route(
            "/api/rmm/chat/session/:session_id/viewer-heartbeat",
            post(chat_viewer_heartbeat),
        )
        .route(
            "/api/rmm/chat/session/:session_id/end",
            post(end_chat_session),
        )
        .route("/agent/ws", get(agent_ws))
        .with_state(state.clone())
        .layer(cors_layer(&state.config))
        .layer(TraceLayer::new_for_http());

    info!("talos_server listening on {}", state.config.bind_addr);
    if let Some(ref u) = state.config.relay_url {
        info!(relay_url = %u, "RMM_RELAY_URL loaded (used for new desktop sessions)");
    }
    let listener = TcpListener::bind(state.config.bind_addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}

fn load_dotenv() {
    let mut candidates = Vec::new();
    if let Ok(cwd) = env::current_dir() {
        candidates.push(cwd.join(".env"));
    }
    if let Ok(exe) = env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join("..").join("..").join(".env"));
        }
    }
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    candidates.push(manifest_dir.join("..").join(".env"));

    for path in candidates {
        if path.exists() {
            let _ = dotenvy::from_path(path);
            break;
        }
    }
}

async fn health() -> &'static str {
    "ok"
}

fn is_stale(created_at: Instant, ttl: Duration, now: Instant) -> bool {
    now.duration_since(created_at) > ttl
}

fn is_unattached_session_stale(
    created_at: Instant,
    attached_at: Option<Instant>,
    now: Instant,
) -> bool {
    attached_at.is_none() && is_stale(created_at, UNATTACHED_SESSION_TTL, now)
}

fn instant_to_datetime(value: Instant) -> DateTime<Utc> {
    let elapsed = value.elapsed();
    let chrono_elapsed =
        chrono::Duration::from_std(elapsed).unwrap_or_else(|_| chrono::Duration::seconds(0));
    Utc::now() - chrono_elapsed
}

fn is_presence_live(last_heartbeat_at: Option<Instant>) -> bool {
    let Some(last_heartbeat_at) = last_heartbeat_at else {
        return false;
    };
    last_heartbeat_at.elapsed() <= VIEWER_HEARTBEAT_TTL
}

fn viewer_state_from_heartbeat(last_heartbeat_at: Option<Instant>) -> ViewerSessionLaunchState {
    if is_presence_live(last_heartbeat_at) {
        ViewerSessionLaunchState::Connected
    } else {
        ViewerSessionLaunchState::Pending
    }
}

async fn notify_agent_session_end(state: &AppState, agent_id: &str, session_id: &str, kind: &str) {
    let agent = {
        let agents = state.agents.read().await;
        agents.get(agent_id).cloned()
    };
    if let Some(agent) = agent {
        let payload = serde_json::json!({
            "session_id": session_id,
            "kind": kind,
        });
        let envelope = OutgoingEnvelope {
            message_type: "session_end",
            data: payload,
        };
        if let Ok(message) = serde_json::to_string(&envelope) {
            let _ = agent.sender.send(Message::Text(message)).await;
        }
    }
}

fn spawn_session_reaper(state: AppState) {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(SESSION_REAPER_INTERVAL).await;
            let now = Instant::now();

            let pending_shell_removed = {
                let mut guard = state.shell_sessions.write().await;
                let before = guard.len();
                guard.retain(|_, pending| !is_stale(pending.created_at, PENDING_REQUEST_TTL, now));
                before.saturating_sub(guard.len())
            };
            let pending_detail_removed = {
                let mut guard = state.detail_requests.write().await;
                let before = guard.len();
                guard.retain(|_, pending| !is_stale(pending.created_at, PENDING_REQUEST_TTL, now));
                before.saturating_sub(guard.len())
            };
            let pending_shell_command_removed = {
                let mut guard = state.shell_commands.write().await;
                let before = guard.len();
                guard.retain(|_, pending| !is_stale(pending.created_at, PENDING_REQUEST_TTL, now));
                before.saturating_sub(guard.len())
            };
            let pending_rdp_removed = {
                let mut guard = state.rdp_sessions_requests.write().await;
                let before = guard.len();
                guard.retain(|_, pending| !is_stale(pending.created_at, PENDING_REQUEST_TTL, now));
                before.saturating_sub(guard.len())
            };
            let pending_capability_removed = {
                let mut guard = state.capability_requests.write().await;
                let before = guard.len();
                guard.retain(|_, pending| !is_stale(pending.created_at, PENDING_REQUEST_TTL, now));
                before.saturating_sub(guard.len())
            };
            let pending_reflex_removed = {
                let mut guard = state.quic_reflex_requests.write().await;
                let before = guard.len();
                guard.retain(|_, pending| !is_stale(pending.created_at, PENDING_REQUEST_TTL, now));
                before.saturating_sub(guard.len())
            };
            let desktop_removed = {
                let mut guard = state.remote_desktop_sessions.write().await;
                let before = guard.len();
                guard.retain(|_, session| {
                    !is_unattached_session_stale(session.created_at, session.attached_at, now)
                });
                before.saturating_sub(guard.len())
            };
            let file_transfer_removed = {
                let mut guard = state.file_transfer_sessions.write().await;
                let before = guard.len();
                guard.retain(|_, session| {
                    !is_unattached_session_stale(session.created_at, session.attached_at, now)
                });
                before.saturating_sub(guard.len())
            };
            let registry_removed = {
                let mut guard = state.remote_registry_sessions.write().await;
                let before = guard.len();
                guard.retain(|_, session| {
                    !is_unattached_session_stale(session.created_at, session.attached_at, now)
                });
                before.saturating_sub(guard.len())
            };
            let stale_chat_sessions = {
                let mut guard = state.chat_sessions.write().await;
                let stale_ids = guard
                    .iter()
                    .filter_map(|(session_id, session)| {
                        let unattached = is_unattached_session_stale(
                            session.created_at,
                            session.attached_at,
                            now,
                        );
                        let heartbeat_expired = session.attached_at.is_some()
                            && session
                                .viewer_last_heartbeat_at
                                .map(|last| last.elapsed() > VIEWER_HEARTBEAT_TTL)
                                .unwrap_or(false);
                        if unattached || heartbeat_expired {
                            Some((session_id.clone(), session.agent_id.clone()))
                        } else {
                            None
                        }
                    })
                    .collect::<Vec<_>>();
                for (session_id, _) in &stale_ids {
                    guard.remove(session_id);
                }
                stale_ids
            };
            for (session_id, agent_id) in &stale_chat_sessions {
                notify_agent_session_end(&state, agent_id, session_id, "chat").await;
            }
            let chat_removed = stale_chat_sessions.len();
            let shell_removed = {
                let mut guard = state.active_shell_sessions.write().await;
                let before = guard.len();
                guard.retain(|_, session| {
                    !is_unattached_session_stale(session.created_at, session.attached_at, now)
                });
                before.saturating_sub(guard.len())
            };

            let removed_total = pending_shell_removed
                + pending_detail_removed
                + pending_shell_command_removed
                + pending_rdp_removed
                + pending_capability_removed
                + pending_reflex_removed
                + desktop_removed
                + file_transfer_removed
                + registry_removed
                + chat_removed
                + shell_removed;

            if removed_total > 0 {
                info!(
                    pending_shell_removed,
                    pending_detail_removed,
                    pending_shell_command_removed,
                    pending_rdp_removed,
                    pending_capability_removed,
                    pending_reflex_removed,
                    desktop_removed,
                    file_transfer_removed,
                    registry_removed,
                    chat_removed,
                    shell_removed,
                    "session reaper removed stale pending requests or unattached sessions"
                );
            }
        }
    });
}

async fn list_devices(State(state): State<AppState>) -> Result<Json<Vec<DeviceSummary>>, AppError> {
    let _ = state;
    Err(AppError::gone("devices list has moved to the API backend"))
}

async fn bulk_update_customer(
    State(state): State<AppState>,
    Json(_payload): Json<Value>,
) -> Result<Json<BulkUpdateCustomerResponse>, AppError> {
    let _ = state;
    Err(AppError::gone(
        "bulk customer updates have moved to the API backend",
    ))
}

async fn bulk_delete_devices(
    State(state): State<AppState>,
    Json(_payload): Json<Value>,
) -> Result<Json<DeleteDevicesResponse>, AppError> {
    let _ = state;
    Err(AppError::gone(
        "bulk device deletes have moved to the API backend",
    ))
}

async fn get_device(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
) -> Result<Json<DeviceSummary>, AppError> {
    let _ = (state, agent_id);
    Err(AppError::gone(
        "device details have moved to the API backend",
    ))
}

async fn delete_device(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
) -> Result<Json<DeleteDevicesResponse>, AppError> {
    let _ = (state, agent_id);
    Err(AppError::gone(
        "device deletes have moved to the API backend",
    ))
}

async fn fetch_device_details(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
) -> Result<Json<DeviceSummary>, AppError> {
    let device = fetch_device_details_for_agent(&state, &agent_id).await?;
    Ok(Json(device))
}

const SNAPSHOT_COOLDOWN_SECS: u64 = 30;

async fn request_snapshot(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    let now = Instant::now();
    {
        let mut last = state.last_snapshot_request.write().await;
        if let Some(&t) = last.get(&agent_id) {
            if now.duration_since(t).as_secs() < SNAPSHOT_COOLDOWN_SECS {
                return Err(AppError::bad_request(
                    "snapshot request is limited to once every 30 seconds",
                ));
            }
        }
        last.insert(agent_id.clone(), now);
    }

    let agent = {
        let agents = state.agents.read().await;
        agents.get(&agent_id).cloned()
    };

    let Some(agent) = agent else {
        return Err(AppError::not_found("agent not connected"));
    };

    let request_id = Uuid::new_v4().to_string();
    let register_path = format!("/rmm/devices/{agent_id}/snapshot-requests");
    let register_body = serde_json::json!({
        "requestId": request_id,
        "status": "pending",
    });
    let _: ApiSnapshotRequestStatusResponse = api_request(
        &state,
        reqwest::Method::POST,
        &register_path,
        None,
        Some(register_body),
        true,
        "register snapshot request",
    )
    .await?;

    let envelope = OutgoingEnvelope {
        message_type: "request_full_snapshot",
        data: RequestFullSnapshotPayload {
            snapshot_request_id: Some(request_id.clone()),
        },
    };
    let message = serde_json::to_string(&envelope).context("serialize request_full_snapshot")?;
    let send_result = agent
        .sender
        .send(Message::Text(message))
        .await
        .map_err(|_| AppError::bad_request("agent unavailable"));
    if send_result.is_err() {
        let failed_body = serde_json::json!({
            "requestId": request_id,
            "status": "failed",
        });
        let _ = api_request::<ApiSnapshotRequestStatusResponse>(
            &state,
            reqwest::Method::POST,
            &register_path,
            None,
            Some(failed_body),
            true,
            "mark snapshot request failed",
        )
        .await;
        return Err(AppError::bad_request("agent unavailable"));
    }

    Ok((
        StatusCode::ACCEPTED,
        Json(serde_json::json!({
            "requestId": request_id,
            "status": "pending",
        })),
    ))
}

async fn fetch_device_details_for_agent(
    state: &AppState,
    agent_id: &str,
) -> Result<DeviceSummary, AppError> {
    let agent = {
        let agents = state.agents.read().await;
        agents.get(agent_id).cloned()
    };

    let Some(agent) = agent else {
        return Err(AppError::not_found("agent not connected"));
    };

    let request_id = Uuid::new_v4().to_string();
    let (response_tx, response_rx) = oneshot::channel();
    {
        let mut requests = state.detail_requests.write().await;
        requests.insert(
            request_id.clone(),
            PendingDetailRequest {
                response_tx,
                created_at: Instant::now(),
            },
        );
    }

    let payload = FetchDetailsPayload {
        request_id: request_id.clone(),
    };
    let envelope = OutgoingEnvelope {
        message_type: "fetch_details",
        data: payload,
    };
    let message = serde_json::to_string(&envelope).context("serialize fetch_details")?;
    agent
        .sender
        .send(Message::Text(message))
        .await
        .map_err(|_| AppError::bad_request("agent unavailable"))?;

    let details = match timeout(Duration::from_secs(15), response_rx).await {
        Ok(Ok(details)) => details,
        _ => {
            let mut requests = state.detail_requests.write().await;
            requests.remove(&request_id);
            return Err(AppError::timeout("device details request timed out"));
        }
    };

    update_device_details(state, agent_id, &details)
        .await
        .map_err(AppError::from)
}

async fn fetch_device_summary_from_api(
    state: &AppState,
    agent_id: &str,
) -> Result<DeviceSummary, AppError> {
    let path = format!("/rmm/devices/{agent_id}");
    let device: ApiDeviceSummary = api_request(
        state,
        reqwest::Method::GET,
        &path,
        None,
        None,
        true,
        "fetch device summary",
    )
    .await?;
    Ok(device.into())
}

async fn execute_script(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    headers: HeaderMap,
    Json(request): Json<ExecuteScriptRequest>,
) -> Result<Json<ScriptResponse>, AppError> {
    let script = request.script;
    if script.trim().is_empty() {
        return Err(AppError::bad_request("script must not be empty"));
    }

    let user_context = require_user_context(&state, &headers).await?;
    ensure_device_in_organization(&state, &agent_id, &user_context.organization_id).await?;
    let device_scope = fetch_device_scope(&state, &agent_id).await?;
    if device_scope.organization_id != user_context.organization_id {
        return Err(AppError::forbidden(
            "device does not belong to your organization",
        ));
    }

    let validation_response: ApiValidationResponse = api_request(
        &state,
        reqwest::Method::POST,
        "/policies/validate",
        None,
        Some(serde_json::json!({
            "command": script.clone(),
            "organizationId": user_context.organization_id.clone(),
            "customerId": device_scope.customer_id.clone(),
            "role": user_context.role.clone(),
        })),
        true,
        "validate command policy",
    )
    .await?;

    let matched_policy_id = validation_response
        .matched_policy_id
        .as_ref()
        .and_then(|value| value.parse::<i64>().ok());
    let reason = validation_response
        .reason
        .unwrap_or_else(|| "Command not allowed".to_string());

    if !validation_response.allowed {
        log_command_execution(
            &state,
            &user_context.organization_id,
            device_scope.customer_id.as_deref(),
            device_scope.site_id.as_deref(),
            &user_context.user_id,
            user_context.email.as_deref(),
            &agent_id,
            &script,
            false,
            Some(&reason),
            matched_policy_id,
            None,
            None,
            None,
        )
        .await?;
        return Err(AppError::forbidden(&reason));
    }
    let agent = {
        let agents = state.agents.read().await;
        agents.get(&agent_id).cloned()
    };

    let Some(agent) = agent else {
        return Err(AppError::not_found("agent not connected"));
    };

    let request_id = Uuid::new_v4().to_string();
    let (response_tx, response_rx) = oneshot::channel();
    {
        let mut commands = state.shell_commands.write().await;
        commands.insert(
            request_id.clone(),
            PendingShellCommand {
                response_tx,
                created_at: Instant::now(),
            },
        );
    }

    let payload = ShellCommandPayload {
        request_id: request_id.clone(),
        command: script.clone(),
    };
    let envelope = OutgoingEnvelope {
        message_type: "shell_command",
        data: payload,
    };
    let message = serde_json::to_string(&envelope).context("serialize shell_command")?;
    agent
        .sender
        .send(Message::Text(message))
        .await
        .map_err(|_| AppError::bad_request("agent unavailable"))?;

    let start = std::time::Instant::now();
    let output = match timeout(
        Duration::from_secs(state.config.max_execution_secs),
        response_rx,
    )
    .await
    {
        Ok(Ok(output)) => output,
        _ => {
            let mut commands = state.shell_commands.write().await;
            commands.remove(&request_id);
            return Err(AppError::timeout("script execution timed out"));
        }
    };

    let elapsed_ms = start.elapsed().as_millis() as i32;
    let (output_text, output_len) = truncate_output(&output.output, state.config.max_output_bytes);

    log_command_execution(
        &state,
        &user_context.organization_id,
        device_scope.customer_id.as_deref(),
        device_scope.site_id.as_deref(),
        &user_context.user_id,
        user_context.email.as_deref(),
        &agent_id,
        &script,
        true,
        None,
        matched_policy_id,
        Some(elapsed_ms),
        Some(output_len),
        output.exit_code,
    )
    .await?;

    Ok(Json(ScriptResponse {
        output: output_text,
        exit_code: output.exit_code,
    }))
}

async fn notify_patch_jobs_for_device(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<PatchJobsNotifyResponse>, AppError> {
    let user_context = require_user_context(&state, &headers).await?;
    ensure_device_in_organization(&state, &agent_id, &user_context.organization_id).await?;
    let device_scope = fetch_device_scope(&state, &agent_id).await?;
    if device_scope.organization_id != user_context.organization_id {
        return Err(AppError::forbidden(
            "device does not belong to your organization",
        ));
    }

    let payload = PatchJobsAvailablePayload {
        reason: "manual".to_string(),
        requested_by: Some(user_context.user_id.clone()),
    };
    let target_name = {
        let agents = state.agents.read().await;
        agents
            .get(&agent_id)
            .and_then(|agent| agent.hostname.clone())
    };
    let notified = send_patch_jobs_available_to_agent(&state, &agent_id, payload).await?;
    let viewer_identity = ViewerIdentity {
        user_id: user_context.user_id.clone(),
        user_email: user_context.email.clone(),
    };
    log_audit_event(
        &state,
        Some(&user_context.organization_id),
        device_scope.customer_id.as_deref(),
        device_scope.site_id.as_deref(),
        Some(&agent_id),
        Some(&viewer_identity),
        "patch.jobs.notify",
        "rmm_device",
        Some(&agent_id),
        target_name.as_deref(),
        if notified { "success" } else { "failure" },
        None,
        serde_json::json!({
            "reason": "manual",
            "notified": notified,
        }),
    )
    .await?;
    Ok(Json(PatchJobsNotifyResponse {
        connected_agents: usize::from(notified),
        notified_agents: usize::from(notified),
    }))
}

async fn notify_patch_jobs_internal(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<PatchJobsNotifyRequest>,
) -> Result<Json<PatchJobsNotifyResponse>, AppError> {
    require_internal_server_key(&state.config, &headers)?;

    let payload = PatchJobsAvailablePayload {
        reason: body
            .reason
            .unwrap_or_else(|| "patch_jobs_available".to_string()),
        requested_by: body.requested_by,
    };
    let target_agent_ids = body
        .agent_ids
        .unwrap_or_default()
        .into_iter()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();

    let agents = {
        let agents = state.agents.read().await;
        if target_agent_ids.is_empty() {
            agents.keys().cloned().collect::<Vec<_>>()
        } else {
            target_agent_ids
                .into_iter()
                .filter(|agent_id| agents.contains_key(agent_id))
                .collect::<Vec<_>>()
        }
    };

    let connected_agents = agents.len();
    let mut notified_agents = 0usize;
    for agent_id in agents {
        if send_patch_jobs_available_to_agent(&state, &agent_id, payload.clone())
            .await
            .unwrap_or(false)
        {
            notified_agents += 1;
        }
    }

    Ok(Json(PatchJobsNotifyResponse {
        connected_agents,
        notified_agents,
    }))
}

async fn send_patch_jobs_available_to_agent(
    state: &AppState,
    agent_id: &str,
    payload: PatchJobsAvailablePayload,
) -> Result<bool, AppError> {
    let agent = {
        let agents = state.agents.read().await;
        agents.get(agent_id).cloned()
    };
    let Some(agent) = agent else {
        return Ok(false);
    };

    let envelope = OutgoingEnvelope {
        message_type: "patch_jobs_available",
        data: payload,
    };
    let message = serde_json::to_string(&envelope).context("serialize patch_jobs_available")?;
    agent
        .sender
        .send(Message::Text(message))
        .await
        .map_err(|_| AppError::bad_request("agent unavailable"))?;
    Ok(true)
}

async fn notify_feature_upgrade_preflight_internal(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<FeatureUpgradePreflightNotifyRequest>,
) -> Result<Json<PatchJobsNotifyResponse>, AppError> {
    require_internal_server_key(&state.config, &headers)?;

    let payload = FeatureUpgradePreflightJobsAvailablePayload {
        reason: body
            .reason
            .unwrap_or_else(|| "feature_upgrade_preflight_available".to_string()),
        requested_by: body.requested_by,
    };
    let target_agent_ids = body
        .agent_ids
        .unwrap_or_default()
        .into_iter()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();

    let agents = {
        let agents = state.agents.read().await;
        if target_agent_ids.is_empty() {
            agents.keys().cloned().collect::<Vec<_>>()
        } else {
            target_agent_ids
                .into_iter()
                .filter(|agent_id| agents.contains_key(agent_id))
                .collect::<Vec<_>>()
        }
    };

    let connected_agents = agents.len();
    let mut notified_agents = 0usize;
    for agent_id in agents {
        if send_feature_upgrade_preflight_jobs_available_to_agent(
            &state,
            &agent_id,
            payload.clone(),
        )
        .await
        .unwrap_or(false)
        {
            notified_agents += 1;
        }
    }

    Ok(Json(PatchJobsNotifyResponse {
        connected_agents,
        notified_agents,
    }))
}

async fn send_feature_upgrade_preflight_jobs_available_to_agent(
    state: &AppState,
    agent_id: &str,
    payload: FeatureUpgradePreflightJobsAvailablePayload,
) -> Result<bool, AppError> {
    let agent = {
        let agents = state.agents.read().await;
        agents.get(agent_id).cloned()
    };
    let Some(agent) = agent else {
        return Ok(false);
    };

    let envelope = OutgoingEnvelope {
        message_type: "feature_upgrade_preflight_jobs_available",
        data: payload,
    };
    let message = serde_json::to_string(&envelope)
        .context("serialize feature_upgrade_preflight_jobs_available")?;
    agent
        .sender
        .send(Message::Text(message))
        .await
        .map_err(|_| AppError::bad_request("agent unavailable"))?;
    Ok(true)
}

async fn notify_feature_upgrade_stage_iso_internal(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<FeatureUpgradeStageIsoNotifyRequest>,
) -> Result<Json<PatchJobsNotifyResponse>, AppError> {
    require_internal_server_key(&state.config, &headers)?;

    let payload = FeatureUpgradeStageIsoJobsAvailablePayload {
        reason: body
            .reason
            .unwrap_or_else(|| "feature_upgrade_stage_iso_available".to_string()),
        requested_by: body.requested_by,
    };
    let target_agent_ids = body
        .agent_ids
        .unwrap_or_default()
        .into_iter()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();

    let agents = {
        let agents = state.agents.read().await;
        if target_agent_ids.is_empty() {
            agents.keys().cloned().collect::<Vec<_>>()
        } else {
            target_agent_ids
                .into_iter()
                .filter(|agent_id| agents.contains_key(agent_id))
                .collect::<Vec<_>>()
        }
    };

    let connected_agents = agents.len();
    let mut notified_agents = 0usize;
    for agent_id in agents {
        if send_feature_upgrade_stage_iso_jobs_available_to_agent(
            &state,
            &agent_id,
            payload.clone(),
        )
        .await
        .unwrap_or(false)
        {
            notified_agents += 1;
        }
    }

    Ok(Json(PatchJobsNotifyResponse {
        connected_agents,
        notified_agents,
    }))
}

async fn send_feature_upgrade_stage_iso_jobs_available_to_agent(
    state: &AppState,
    agent_id: &str,
    payload: FeatureUpgradeStageIsoJobsAvailablePayload,
) -> Result<bool, AppError> {
    let agent = {
        let agents = state.agents.read().await;
        agents.get(agent_id).cloned()
    };
    let Some(agent) = agent else {
        return Ok(false);
    };

    let envelope = OutgoingEnvelope {
        message_type: "feature_upgrade_stage_iso_jobs_available",
        data: payload,
    };
    let message = serde_json::to_string(&envelope)
        .context("serialize feature_upgrade_stage_iso_jobs_available")?;
    agent
        .sender
        .send(Message::Text(message))
        .await
        .map_err(|_| AppError::bad_request("agent unavailable"))?;
    Ok(true)
}

async fn notify_feature_upgrade_start_internal(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<FeatureUpgradeStartNotifyRequest>,
) -> Result<Json<PatchJobsNotifyResponse>, AppError> {
    require_internal_server_key(&state.config, &headers)?;

    let payload = FeatureUpgradeStartJobsAvailablePayload {
        reason: body
            .reason
            .unwrap_or_else(|| "feature_upgrade_start_available".to_string()),
        requested_by: body.requested_by,
    };
    let target_agent_ids = body
        .agent_ids
        .unwrap_or_default()
        .into_iter()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();

    let agents = {
        let agents = state.agents.read().await;
        if target_agent_ids.is_empty() {
            agents.keys().cloned().collect::<Vec<_>>()
        } else {
            target_agent_ids
                .into_iter()
                .filter(|agent_id| agents.contains_key(agent_id))
                .collect::<Vec<_>>()
        }
    };

    let connected_agents = agents.len();
    let mut notified_agents = 0usize;
    for agent_id in agents {
        if send_feature_upgrade_start_jobs_available_to_agent(&state, &agent_id, payload.clone())
            .await
            .unwrap_or(false)
        {
            notified_agents += 1;
        }
    }

    Ok(Json(PatchJobsNotifyResponse {
        connected_agents,
        notified_agents,
    }))
}

async fn send_feature_upgrade_start_jobs_available_to_agent(
    state: &AppState,
    agent_id: &str,
    payload: FeatureUpgradeStartJobsAvailablePayload,
) -> Result<bool, AppError> {
    let agent = {
        let agents = state.agents.read().await;
        agents.get(agent_id).cloned()
    };
    let Some(agent) = agent else {
        return Ok(false);
    };

    let envelope = OutgoingEnvelope {
        message_type: "feature_upgrade_start_jobs_available",
        data: payload,
    };
    let message = serde_json::to_string(&envelope)
        .context("serialize feature_upgrade_start_jobs_available")?;
    agent
        .sender
        .send(Message::Text(message))
        .await
        .map_err(|_| AppError::bad_request("agent unavailable"))?;
    Ok(true)
}

async fn enqueue_remediation_commands_internal(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<RemediationCommandsEnqueueRequest>,
) -> Result<Json<RemediationCommandsEnqueueResponse>, AppError> {
    require_internal_server_key(&state.config, &headers)?;

    let (queued, target_agents) = notification_targets(body.commands);
    let connected_agents = {
        let agents = state.agents.read().await;
        target_agents
            .iter()
            .filter(|agent_id| agents.contains_key(*agent_id))
            .count()
    };
    let mut notified_agents = 0usize;
    for agent_id in target_agents {
        if send_remediation_jobs_available_to_agent(
            &state,
            &agent_id,
            RemediationJobsAvailablePayload {
                reason: "remediation_command_queued".to_string(),
                requested_by: None,
            },
        )
        .await
        .unwrap_or(false)
        {
            notified_agents += 1;
        }
    }

    Ok(Json(RemediationCommandsEnqueueResponse {
        accepted: true,
        queued,
        connected_agents,
        notified_agents,
    }))
}

async fn send_remediation_jobs_available_to_agent(
    state: &AppState,
    agent_id: &str,
    payload: RemediationJobsAvailablePayload,
) -> Result<bool, AppError> {
    let agent = {
        let agents = state.agents.read().await;
        agents.get(agent_id).cloned()
    };
    let Some(agent) = agent else {
        return Ok(false);
    };

    let envelope = OutgoingEnvelope {
        message_type: "remediation_jobs_available",
        data: payload,
    };
    let message =
        serde_json::to_string(&envelope).context("serialize remediation_jobs_available")?;
    agent
        .sender
        .send(Message::Text(message))
        .await
        .map_err(|_| AppError::bad_request("agent unavailable"))?;
    Ok(true)
}

async fn claim_remediation_jobs_for_agent(
    state: &AppState,
    agent_id: &str,
    limit: u32,
) -> Result<Vec<RemediationCommandJob>, AppError> {
    let response: ApiRemediationJobsClaimResponse = api_request(
        state,
        reqwest::Method::POST,
        &format!("/rmm/telemetry/remediation/agents/{agent_id}/jobs/claim"),
        None,
        Some(serde_json::json!({ "limit": limit.clamp(1, 10) })),
        true,
        "claim remediation jobs",
    )
    .await?;
    Ok(response.jobs)
}

async fn publish_remediation_status_for_agent(
    state: &AppState,
    agent_id: &str,
    payload: RemediationJobUpdatePayload,
) -> Result<(), AppError> {
    let command_id = payload.command_id.clone();
    let expected_status = payload.status.clone();
    let response: ApiRemediationJobStatusResponse = api_request(
        state,
        reqwest::Method::PATCH,
        &format!("/rmm/telemetry/remediation/agents/{agent_id}/jobs/{command_id}/status"),
        None,
        Some(status_request_body(&payload)),
        true,
        "report remediation job status",
    )
    .await?;
    if !response.updated || response.status != expected_status {
        return Err(AppError::internal(
            "telemetry api did not persist remediation status",
        ));
    }
    Ok(())
}

async fn claim_patch_jobs_for_agent(
    state: &AppState,
    agent_id: &str,
    limit: u32,
) -> Result<Vec<PatchRemediationJob>, AppError> {
    let response: ApiPatchJobsClaimResponse = api_request(
        state,
        reqwest::Method::POST,
        &format!("/rmm/telemetry/remediation/agents/{agent_id}/patch-jobs/claim"),
        None,
        Some(serde_json::json!({ "limit": limit.clamp(1, 3) })),
        true,
        "claim patch jobs",
    )
    .await?;
    Ok(response.jobs)
}

async fn report_patch_job_update_for_agent(
    state: &AppState,
    agent_id: &str,
    payload: PatchJobUpdatePayload,
) -> Result<(), AppError> {
    let job_id = payload.job_id.clone();
    let status = payload.status.clone();
    let body = serde_json::json!({
        "status": status,
        "stepIndex": payload.step_index.unwrap_or(0),
        "evidence": payload.evidence,
    });
    let response: ApiPatchJobStatusResponse = api_request(
        state,
        reqwest::Method::PATCH,
        &format!(
            "/rmm/telemetry/remediation/agents/{agent_id}/patch-jobs/{}/status",
            job_id
        ),
        None,
        Some(body),
        true,
        "report patch job update",
    )
    .await?;
    if !response.updated {
        warn!(
            agent_id,
            job_id = %job_id,
            status = %response.status,
            "patch job update was not accepted"
        );
    }
    Ok(())
}

async fn publish_patch_progress_for_agent(
    state: &AppState,
    connected_agent_id: &str,
    payload: PatchJobProgressPayload,
) -> Result<(), AppError> {
    if payload.agent_id != connected_agent_id {
        warn!(
            connected_agent_id,
            payload_agent_id = %payload.agent_id,
            "rejecting patch progress for a different agent"
        );
        return Ok(());
    }
    let mut record = payload.extra;
    record.insert(
        "organizationId".to_string(),
        Value::String(payload.organization_id),
    );
    record.insert("agentId".to_string(), Value::String(payload.agent_id));
    record.insert("jobId".to_string(), Value::String(payload.job_id));
    record.insert("commandId".to_string(), Value::String(payload.command_id));
    record.insert("status".to_string(), Value::String(payload.status));
    record.insert("phase".to_string(), Value::String(payload.phase));
    let progress_value = Value::Object(record);
    let url = state.config.patch_progress_url();
    let response: ApiAccepted = api_request_with_url(
        state,
        reqwest::Method::POST,
        &url,
        None,
        Some(serde_json::json!({ "progress": [progress_value] })),
        true,
        "publish patch progress",
    )
    .await?;
    if !response.accepted {
        return Err(AppError::internal(
            "patch progress destination did not accept progress",
        ));
    }
    Ok(())
}

async fn claim_feature_upgrade_preflight_jobs_for_agent(
    state: &AppState,
    agent_id: &str,
    limit: u32,
) -> Result<Vec<Value>, AppError> {
    let response: ApiFeatureUpgradePreflightJobsClaimResponse = api_request(
        state,
        reqwest::Method::POST,
        "/rmm/feature-upgrades/internal/preflight/jobs/claim",
        None,
        Some(serde_json::json!({ "agentId": agent_id, "limit": limit.clamp(1, 3) })),
        true,
        "claim feature upgrade preflight jobs",
    )
    .await?;
    Ok(response.jobs)
}

async fn publish_feature_upgrade_preflight_progress_for_agent(
    state: &AppState,
    connected_agent_id: &str,
    payload: FeatureUpgradePreflightProgressPayload,
) -> Result<(), AppError> {
    if payload.agent_id != connected_agent_id {
        warn!(
            connected_agent_id,
            payload_agent_id = %payload.agent_id,
            "rejecting feature upgrade preflight progress for a different agent"
        );
        return Ok(());
    }
    let body = serde_json::to_value(payload)
        .map_err(|error| AppError::internal(&format!("serialize preflight progress: {error}")))?;
    let response: ApiFeatureUpgradePreflightProgressResponse = api_request(
        state,
        reqwest::Method::POST,
        "/rmm/feature-upgrades/internal/preflight/progress",
        None,
        Some(body),
        true,
        "publish feature upgrade preflight progress",
    )
    .await?;
    if !response.accepted {
        return Err(AppError::internal(
            "api backend did not accept feature upgrade preflight progress",
        ));
    }
    if response.updated == 0 {
        warn!("feature upgrade preflight progress did not update any rows");
    }
    Ok(())
}

async fn claim_feature_upgrade_stage_iso_jobs_for_agent(
    state: &AppState,
    agent_id: &str,
    limit: u32,
) -> Result<Vec<Value>, AppError> {
    let response: ApiFeatureUpgradeStageIsoJobsClaimResponse = api_request(
        state,
        reqwest::Method::POST,
        "/rmm/feature-upgrades/internal/stage-iso/jobs/claim",
        None,
        Some(serde_json::json!({ "agentId": agent_id, "limit": limit.clamp(1, 3) })),
        true,
        "claim feature upgrade stage ISO jobs",
    )
    .await?;
    Ok(response.jobs)
}

async fn publish_feature_upgrade_stage_iso_progress_for_agent(
    state: &AppState,
    connected_agent_id: &str,
    payload: FeatureUpgradeStageIsoProgressPayload,
) -> Result<(), AppError> {
    if payload.agent_id != connected_agent_id {
        warn!(
            connected_agent_id,
            payload_agent_id = %payload.agent_id,
            "rejecting feature upgrade stage ISO progress for a different agent"
        );
        return Ok(());
    }

    let mut record = payload.extra.clone();
    record.insert(
        "operationId".to_string(),
        Value::String(payload.operation_id.clone()),
    );
    record.insert("runId".to_string(), Value::String(payload.run_id.clone()));
    record.insert(
        "organizationId".to_string(),
        Value::String(payload.organization_id.clone()),
    );
    record.insert(
        "agentId".to_string(),
        Value::String(payload.agent_id.clone()),
    );
    record.insert("status".to_string(), Value::String(payload.status.clone()));
    record.insert("phase".to_string(), Value::String(payload.phase.clone()));
    record.insert(
        "receivedAt".to_string(),
        Value::String(Utc::now().to_rfc3339()),
    );
    let progress_value = Value::Object(record);

    let response: ApiFeatureUpgradeStageIsoProgressResponse = api_request(
        state,
        reqwest::Method::POST,
        "/rmm/feature-upgrades/internal/stage-iso/progress",
        None,
        Some(progress_value),
        true,
        "publish feature upgrade stage ISO progress",
    )
    .await?;
    if !response.accepted {
        return Err(AppError::internal(
            "api backend did not accept feature upgrade stage ISO progress",
        ));
    }
    if response.updated == 0 {
        warn!("feature upgrade stage ISO progress did not update any rows");
    }
    Ok(())
}

async fn claim_feature_upgrade_start_jobs_for_agent(
    state: &AppState,
    agent_id: &str,
    limit: u32,
) -> Result<Vec<Value>, AppError> {
    let response: ApiFeatureUpgradeStartJobsClaimResponse = api_request(
        state,
        reqwest::Method::POST,
        "/rmm/feature-upgrades/internal/start/jobs/claim",
        None,
        Some(serde_json::json!({ "agentId": agent_id, "limit": limit.clamp(1, 3) })),
        true,
        "claim feature upgrade start jobs",
    )
    .await?;
    Ok(response.jobs)
}

async fn publish_feature_upgrade_start_progress_for_agent(
    state: &AppState,
    connected_agent_id: &str,
    payload: FeatureUpgradeStartProgressPayload,
) -> Result<(), AppError> {
    if payload.agent_id != connected_agent_id {
        warn!(
            connected_agent_id,
            payload_agent_id = %payload.agent_id,
            "rejecting feature upgrade start progress for a different agent"
        );
        return Ok(());
    }

    let mut record = payload.extra.clone();
    record.insert(
        "operationId".to_string(),
        Value::String(payload.operation_id.clone()),
    );
    record.insert("runId".to_string(), Value::String(payload.run_id.clone()));
    record.insert(
        "organizationId".to_string(),
        Value::String(payload.organization_id.clone()),
    );
    record.insert(
        "agentId".to_string(),
        Value::String(payload.agent_id.clone()),
    );
    record.insert("status".to_string(), Value::String(payload.status.clone()));
    record.insert("phase".to_string(), Value::String(payload.phase.clone()));
    record.insert(
        "receivedAt".to_string(),
        Value::String(Utc::now().to_rfc3339()),
    );
    let progress_value = Value::Object(record);

    let response: ApiFeatureUpgradeStartProgressResponse = api_request(
        state,
        reqwest::Method::POST,
        "/rmm/feature-upgrades/internal/start/progress",
        None,
        Some(progress_value),
        true,
        "publish feature upgrade start progress",
    )
    .await?;
    if !response.accepted {
        return Err(AppError::internal(
            "api backend did not accept feature upgrade start progress",
        ));
    }
    if response.updated == 0 {
        warn!("feature upgrade start progress did not update any rows");
    }
    Ok(())
}

async fn evaluate_patch_state_for_agent(
    state: &AppState,
    agent_id: &str,
    payload: PatchStateCheckinPayload,
) -> Result<Value, AppError> {
    let body = serde_json::json!({
        "agentId": agent_id,
        "observedAt": payload.observed_at.unwrap_or_else(|| Utc::now().to_rfc3339()),
        "state": payload.state
    });
    let response: ApiPatchActionPlanResponse = api_request(
        state,
        reqwest::Method::POST,
        "/rmm/telemetry/patch/checkin",
        None,
        Some(body),
        true,
        "evaluate patch state check-in",
    )
    .await?;
    Ok(response.plan)
}

async fn report_patch_action_result_for_agent(
    state: &AppState,
    agent_id: &str,
    payload: PatchActionResultPayload,
) -> Result<(), AppError> {
    let body = serde_json::json!({
        "agentId": agent_id,
        "operationId": payload.operation_id,
        "action": payload.action,
        "status": payload.status,
        "updateKeys": payload.update_keys,
        "evidence": payload.evidence
    });
    let response: ApiPatchActionResultResponse = api_request(
        state,
        reqwest::Method::POST,
        "/rmm/telemetry/patch/action-result",
        None,
        Some(body),
        true,
        "report patch action result",
    )
    .await?;
    if !response.accepted {
        warn!(agent_id, "patch action result was not accepted");
    }
    Ok(())
}

async fn report_macos_update_account_status_for_agent(
    state: &AppState,
    connected_agent_id: &str,
    payload: MacosUpdateAccountStatusPayload,
) -> Result<(), AppError> {
    if payload.agent_id != connected_agent_id {
        warn!(
            connected_agent_id,
            payload_agent_id = %payload.agent_id,
            "rejecting macOS update account status for a different agent"
        );
        return Ok(());
    }
    let response: ApiAccepted = api_request(
        state,
        reqwest::Method::POST,
        &format!("/rmm/devices/{connected_agent_id}/macos-update-account-status"),
        None,
        Some(serde_json::to_value(payload.status).map_err(|error| {
            AppError::internal(&format!("serialize macOS update account status: {error}"))
        })?),
        true,
        "persist macOS update account status",
    )
    .await?;
    if !response.accepted {
        return Err(AppError::internal(
            "api backend did not accept macOS update account status",
        ));
    }
    Ok(())
}

fn require_internal_server_key(config: &Config, headers: &HeaderMap) -> Result<(), AppError> {
    let Some(expected) = config
        .talos_server_api_key
        .as_ref()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
    else {
        return Err(AppError::service_unavailable(
            "RMM_SERVER_API_KEY must be configured",
        ));
    };

    let presented = headers
        .get("x-rmm-server-key")
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .unwrap_or("");
    if presented != expected {
        return Err(AppError::unauthorized("unauthorized"));
    }
    Ok(())
}

async fn list_rdp_sessions(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
) -> Result<Json<RdpSessionsHttpResponse>, AppError> {
    let agent = {
        let agents = state.agents.read().await;
        agents.get(&agent_id).cloned()
    };

    let Some(agent) = agent else {
        return Err(AppError::not_found("agent not connected"));
    };

    let request_id = Uuid::new_v4().to_string();
    let (response_tx, response_rx) = oneshot::channel();
    {
        let mut requests = state.rdp_sessions_requests.write().await;
        requests.insert(
            request_id.clone(),
            PendingRdpSessionsRequest {
                response_tx,
                created_at: Instant::now(),
            },
        );
    }

    let payload = RdpSessionsRequestPayload {
        request_id: request_id.clone(),
    };
    let envelope = OutgoingEnvelope {
        message_type: "rdp_sessions_request",
        data: payload,
    };
    let message = serde_json::to_string(&envelope).context("serialize rdp_sessions_request")?;
    agent
        .sender
        .send(Message::Text(message))
        .await
        .map_err(|_| AppError::bad_request("agent unavailable"))?;

    let sessions = match timeout(Duration::from_secs(10), response_rx).await {
        Ok(Ok(sessions)) => sessions,
        _ => {
            let mut requests = state.rdp_sessions_requests.write().await;
            requests.remove(&request_id);
            return Err(AppError::timeout("rdp session enumeration timed out"));
        }
    };

    Ok(Json(RdpSessionsHttpResponse { sessions }))
}

#[derive(Clone)]
struct DeviceScope {
    organization_id: String,
    customer_id: Option<String>,
    site_id: Option<String>,
}

#[derive(Clone)]
struct ViewerSessionSnapshot {
    session_id: String,
    kind: ViewerSessionKind,
    agent_id: String,
    user_id: Option<String>,
    user_email: Option<String>,
    attached_at: Option<Instant>,
    viewer_connected_at: Option<Instant>,
    viewer_last_heartbeat_at: Option<Instant>,
}

async fn require_user_context(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<UserContext, AppError> {
    let token =
        extract_bearer(headers).ok_or_else(|| AppError::unauthorized("missing bearer token"))?;

    let context: ApiUserContext = api_request(
        state,
        reqwest::Method::GET,
        "/auth/context",
        Some(&token),
        None,
        false,
        "fetch user context",
    )
    .await?;

    Ok(UserContext {
        user_id: context.user_id,
        organization_id: context.organization_id,
        role: context.role,
        email: context.email,
    })
}

async fn ensure_device_in_organization(
    state: &AppState,
    agent_id: &str,
    organization_id: &str,
) -> Result<(), AppError> {
    let path = format!("/rmm/devices/{agent_id}/ensure-org");
    let body = serde_json::json!({
        "organizationId": organization_id,
    });

    let _: serde_json::Value = api_request(
        state,
        reqwest::Method::POST,
        &path,
        None,
        Some(body),
        true,
        "ensure device in organization",
    )
    .await?;

    Ok(())
}

async fn fetch_device_scope(state: &AppState, agent_id: &str) -> Result<DeviceScope, AppError> {
    let path = format!("/rmm/devices/{agent_id}/scope");
    let response: ApiDeviceScope = api_request(
        state,
        reqwest::Method::GET,
        &path,
        None,
        None,
        true,
        "fetch device scope",
    )
    .await?;

    let organization_id = response.organization_id.trim().to_string();
    if organization_id.is_empty() {
        return Err(AppError::forbidden(
            "device is not assigned to an organization",
        ));
    }

    Ok(DeviceScope {
        organization_id,
        customer_id: response.customer_id,
        site_id: response.site_id,
    })
}

fn viewer_status_response_from_snapshot(
    snapshot: ViewerSessionSnapshot,
) -> ViewerSessionStatusResponse {
    let state = viewer_state_from_heartbeat(snapshot.viewer_last_heartbeat_at);
    ViewerSessionStatusResponse {
        session_id: snapshot.session_id,
        kind: snapshot.kind,
        agent_id: snapshot.agent_id,
        user_id: snapshot.user_id,
        user_email: snapshot.user_email,
        state,
        connected: state == ViewerSessionLaunchState::Connected,
        attached: snapshot.attached_at.is_some(),
        connected_at: snapshot.viewer_connected_at.map(instant_to_datetime),
        last_heartbeat_at: snapshot.viewer_last_heartbeat_at.map(instant_to_datetime),
    }
}

fn viewer_connection_summary_from_snapshot(
    snapshot: ViewerSessionSnapshot,
) -> ViewerConnectionSummary {
    ViewerConnectionSummary {
        session_id: snapshot.session_id,
        kind: snapshot.kind,
        agent_id: snapshot.agent_id,
        user_id: snapshot.user_id,
        user_email: snapshot.user_email,
        connected_at: snapshot.viewer_connected_at.map(instant_to_datetime),
        last_heartbeat_at: snapshot.viewer_last_heartbeat_at.map(instant_to_datetime),
    }
}

async fn find_viewer_session_snapshot(
    state: &AppState,
    session_id: &str,
) -> Option<ViewerSessionSnapshot> {
    {
        let sessions = state.remote_desktop_sessions.read().await;
        if let Some(session) = sessions.get(session_id) {
            return Some(ViewerSessionSnapshot {
                session_id: session_id.to_string(),
                kind: ViewerSessionKind::RemoteDesktop,
                agent_id: session.agent_id.clone(),
                user_id: session.viewer_user_id.clone(),
                user_email: session.viewer_user_email.clone(),
                attached_at: session.attached_at,
                viewer_connected_at: session.viewer_connected_at,
                viewer_last_heartbeat_at: session.viewer_last_heartbeat_at,
            });
        }
    }
    {
        let sessions = state.active_shell_sessions.read().await;
        if let Some(session) = sessions.get(session_id) {
            return Some(ViewerSessionSnapshot {
                session_id: session_id.to_string(),
                kind: ViewerSessionKind::Shell,
                agent_id: session.agent_id.clone(),
                user_id: session.viewer_user_id.clone(),
                user_email: session.viewer_user_email.clone(),
                attached_at: session.attached_at,
                viewer_connected_at: session.viewer_connected_at,
                viewer_last_heartbeat_at: session.viewer_last_heartbeat_at,
            });
        }
    }
    {
        let sessions = state.file_transfer_sessions.read().await;
        if let Some(session) = sessions.get(session_id) {
            return Some(ViewerSessionSnapshot {
                session_id: session_id.to_string(),
                kind: ViewerSessionKind::FileTransfer,
                agent_id: session.agent_id.clone(),
                user_id: session.viewer_user_id.clone(),
                user_email: session.viewer_user_email.clone(),
                attached_at: session.attached_at,
                viewer_connected_at: session.viewer_connected_at,
                viewer_last_heartbeat_at: session.viewer_last_heartbeat_at,
            });
        }
    }
    {
        let sessions = state.remote_registry_sessions.read().await;
        if let Some(session) = sessions.get(session_id) {
            return Some(ViewerSessionSnapshot {
                session_id: session_id.to_string(),
                kind: ViewerSessionKind::RemoteRegistry,
                agent_id: session.agent_id.clone(),
                user_id: session.viewer_user_id.clone(),
                user_email: session.viewer_user_email.clone(),
                attached_at: session.attached_at,
                viewer_connected_at: session.viewer_connected_at,
                viewer_last_heartbeat_at: session.viewer_last_heartbeat_at,
            });
        }
    }
    {
        let sessions = state.chat_sessions.read().await;
        if let Some(session) = sessions.get(session_id) {
            return Some(ViewerSessionSnapshot {
                session_id: session_id.to_string(),
                kind: ViewerSessionKind::Chat,
                agent_id: session.agent_id.clone(),
                user_id: session.viewer_user_id.clone(),
                user_email: session.viewer_user_email.clone(),
                attached_at: session.attached_at,
                viewer_connected_at: session.viewer_connected_at,
                viewer_last_heartbeat_at: session.viewer_last_heartbeat_at,
            });
        }
    }
    None
}

async fn enroll_agent(
    state: &AppState,
    registration_token: &str,
    agent_id: &str,
    hostname: &str,
    os: &str,
    ip: &str,
    version: Option<&str>,
) -> Result<ApiEnrollmentResponse, AppError> {
    let body = serde_json::json!({
        "token": registration_token,
        "agentId": agent_id,
        "hostname": hostname,
        "os": os,
        "ip": ip,
        "version": version,
    });

    let response: ApiEnrollmentResponse = api_request(
        state,
        reqwest::Method::POST,
        "/rmm/installers/enroll",
        None,
        Some(body),
        true,
        "enroll agent",
    )
    .await?;

    if !response.enrolled {
        return Err(AppError::forbidden("agent enrollment was rejected"));
    }
    if response.organization_id.trim().is_empty() {
        return Err(AppError::forbidden(
            "agent enrollment did not return organization scope",
        ));
    }
    Ok(response)
}

fn truncate_output(output: &str, max_bytes: usize) -> (String, i32) {
    let bytes = output.as_bytes();
    let suffix = "\n\n[output truncated]";
    if bytes.len() <= max_bytes {
        return (output.to_string(), bytes.len() as i32);
    }

    let limit = max_bytes.saturating_sub(suffix.len());
    let truncated = String::from_utf8_lossy(&bytes[..limit]).to_string();
    let final_output = format!("{truncated}{suffix}");
    (final_output, max_bytes as i32)
}

async fn log_command_execution(
    state: &AppState,
    organization_id: &str,
    customer_id: Option<&str>,
    site_id: Option<&str>,
    user_id: &str,
    user_email: Option<&str>,
    agent_id: &str,
    command: &str,
    was_allowed: bool,
    denial_reason: Option<&str>,
    matched_policy_id: Option<i64>,
    execution_time_ms: Option<i32>,
    output_length: Option<i32>,
    exit_code: Option<i32>,
) -> Result<(), AppError> {
    let body = serde_json::json!({
        "organizationId": organization_id,
        "customerId": customer_id,
        "siteId": site_id,
        "userId": user_id,
        "userEmail": user_email,
        "agentId": agent_id,
        "command": command,
        "wasAllowed": was_allowed,
        "denialReason": denial_reason,
        "matchedPolicyId": matched_policy_id,
        "executionTimeMs": execution_time_ms,
        "outputLength": output_length,
        "exitCode": exit_code,
    });

    let _: serde_json::Value = api_request(
        state,
        reqwest::Method::POST,
        "/rmm/command-log",
        None,
        Some(body),
        true,
        "log command execution",
    )
    .await?;

    Ok(())
}

async fn log_audit_event(
    state: &AppState,
    organization_id: Option<&str>,
    customer_id: Option<&str>,
    site_id: Option<&str>,
    agent_id: Option<&str>,
    viewer_identity: Option<&ViewerIdentity>,
    action_type: &str,
    target_type: &str,
    target_id: Option<&str>,
    target_name: Option<&str>,
    result: &str,
    session_id: Option<&str>,
    metadata: Value,
) -> Result<(), AppError> {
    let body = serde_json::json!({
        "organizationId": organization_id,
        "customerId": customer_id,
        "siteId": site_id,
        "agentId": agent_id,
        "actorType": if viewer_identity.is_some() { "user" } else { "service" },
        "userId": viewer_identity.map(|identity| identity.user_id.as_str()),
        "userEmail": viewer_identity.and_then(|identity| identity.user_email.as_deref()),
        "actionType": action_type,
        "targetType": target_type,
        "targetId": target_id,
        "targetName": target_name,
        "result": result,
        "sessionId": session_id,
        "metadata": metadata,
    });

    let _: serde_json::Value = api_request(
        state,
        reqwest::Method::POST,
        "/audit/events",
        None,
        Some(body),
        true,
        "log audit event",
    )
    .await?;

    Ok(())
}

async fn connect_device(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<ConnectResponse>, AppError> {
    let user_context = require_user_context(&state, &headers).await?;
    ensure_device_in_organization(&state, &agent_id, &user_context.organization_id).await?;
    let device_scope = fetch_device_scope(&state, &agent_id).await?;
    if device_scope.organization_id != user_context.organization_id {
        return Err(AppError::forbidden(
            "device does not belong to your organization",
        ));
    }
    let api_base = extract_api_base(&headers);
    let response = start_remote_desktop_session(
        &state,
        &agent_id,
        api_base,
        "desktop",
        SessionTransportMode::RemoteDesktop,
        Some(ViewerIdentity {
            user_id: user_context.user_id.clone(),
            user_email: user_context.email.clone(),
        }),
        None,
        false,
    )
    .await?;

    Ok(Json(response))
}

async fn connect_file_transfer(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<ConnectResponse>, AppError> {
    let user_context = require_user_context(&state, &headers).await?;
    ensure_device_in_organization(&state, &agent_id, &user_context.organization_id).await?;
    let device_scope = fetch_device_scope(&state, &agent_id).await?;
    if device_scope.organization_id != user_context.organization_id {
        return Err(AppError::forbidden(
            "device does not belong to your organization",
        ));
    }
    let api_base = extract_api_base(&headers);
    let response = start_file_transfer_session(
        &state,
        &agent_id,
        api_base,
        Some(ViewerIdentity {
            user_id: user_context.user_id.clone(),
            user_email: user_context.email.clone(),
        }),
    )
    .await?;

    Ok(Json(response))
}

async fn connect_registry(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<ConnectResponse>, AppError> {
    let user_context = require_user_context(&state, &headers).await?;
    ensure_device_in_organization(&state, &agent_id, &user_context.organization_id).await?;
    let device_scope = fetch_device_scope(&state, &agent_id).await?;
    if device_scope.organization_id != user_context.organization_id {
        return Err(AppError::forbidden(
            "device does not belong to your organization",
        ));
    }
    let api_base = extract_api_base(&headers);
    let response = start_remote_registry_session(
        &state,
        &agent_id,
        api_base,
        Some(ViewerIdentity {
            user_id: user_context.user_id.clone(),
            user_email: user_context.email.clone(),
        }),
    )
    .await?;

    Ok(Json(response))
}

async fn connect_ai_runner_desktop_session_internal(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<InternalAiRunnerConnectRequest>,
) -> Result<Json<ConnectResponse>, AppError> {
    require_internal_server_key(&state.config, &headers)?;
    let organization_id = body.organization_id.trim();
    if organization_id.is_empty() {
        return Err(AppError::bad_request("organizationId is required"));
    }
    let device_scope = fetch_device_scope(&state, &agent_id).await?;
    if device_scope.organization_id != organization_id {
        return Err(AppError::forbidden(
            "device does not belong to the requested organization",
        ));
    }

    let api_base = body
        .api_base_url
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .or_else(|| extract_api_base(&headers));
    let desktop_mode = AiRunnerDesktopMode::from_request(body.desktop_mode.as_deref());
    let session_mode = match desktop_mode {
        AiRunnerDesktopMode::Interactive => SessionTransportMode::RemoteDesktop,
        AiRunnerDesktopMode::ScreenshotOnly => SessionTransportMode::HeadlessRemoteDesktop,
    };
    let display_profile_preference = if desktop_mode == AiRunnerDesktopMode::Interactive {
        Some(body.display_profile_preference.as_slice())
    } else {
        None
    };
    let hide_cursor = body
        .hide_cursor
        .unwrap_or(desktop_mode == AiRunnerDesktopMode::Interactive);
    let response = start_remote_desktop_session(
        &state,
        &agent_id,
        api_base,
        "headless",
        session_mode,
        None,
        display_profile_preference,
        hide_cursor,
    )
    .await?;

    log_audit_event(
        &state,
        Some(&device_scope.organization_id),
        device_scope.customer_id.as_deref(),
        device_scope.site_id.as_deref(),
        Some(&agent_id),
        None,
        "ai_runner.headless_session_lease",
        "rmm_device",
        Some(&agent_id),
        None,
        "success",
        Some(&response.session_id),
        serde_json::json!({
            "jobId": body.job_id,
            "desktopMode": body.desktop_mode,
            "displayProfilePreference": body.display_profile_preference,
            "hideCursor": hide_cursor,
        }),
    )
    .await?;

    Ok(Json(response))
}

async fn connect_ai_runner_chat_approval_internal(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<InternalAiRunnerConnectRequest>,
) -> Result<Json<ConnectResponse>, AppError> {
    require_internal_server_key(&state.config, &headers)?;
    let organization_id = body.organization_id.trim();
    if organization_id.is_empty() {
        return Err(AppError::bad_request("organizationId is required"));
    }
    let device_scope = fetch_device_scope(&state, &agent_id).await?;
    if device_scope.organization_id != organization_id {
        return Err(AppError::forbidden(
            "device does not belong to the requested organization",
        ));
    }

    let api_base = body
        .api_base_url
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .or_else(|| extract_api_base(&headers));
    let response = start_chat_session(&state, &agent_id, api_base, None, None).await?;

    log_audit_event(
        &state,
        Some(&device_scope.organization_id),
        device_scope.customer_id.as_deref(),
        device_scope.site_id.as_deref(),
        Some(&agent_id),
        None,
        "ai_runner.chat_approval_session",
        "rmm_device",
        Some(&agent_id),
        None,
        "success",
        Some(&response.session_id),
        serde_json::json!({
            "jobId": body.job_id,
        }),
    )
    .await?;

    Ok(Json(response))
}

fn remote_desktop_profile_supported(
    capabilities: &RemoteDesktopCapabilities,
    profile_id: &str,
) -> bool {
    capabilities
        .display_profiles
        .iter()
        .any(|profile| profile.id == profile_id)
}

fn choose_remote_desktop_profile(
    capabilities: &RemoteDesktopCapabilities,
    prefer_screenshot_only: bool,
) -> Option<String> {
    let preferred_profiles: &[&str] = if prefer_screenshot_only {
        &[
            REMOTE_DESKTOP_PROFILE_SCREENSHOT_ONLY,
            REMOTE_DESKTOP_PROFILE_MODERN_GPU,
            REMOTE_DESKTOP_PROFILE_LEGACY,
            REMOTE_DESKTOP_PROFILE_MODERN_CPU,
            REMOTE_DESKTOP_PROFILE_EXPERIMENTAL,
        ]
    } else {
        &[
            REMOTE_DESKTOP_PROFILE_MODERN_GPU,
            REMOTE_DESKTOP_PROFILE_LEGACY,
            REMOTE_DESKTOP_PROFILE_MODERN_CPU,
            REMOTE_DESKTOP_PROFILE_EXPERIMENTAL,
        ]
    };

    preferred_profiles
        .iter()
        .find(|candidate| remote_desktop_profile_supported(capabilities, candidate))
        .map(|profile| (*profile).to_string())
        .or_else(|| {
            capabilities
                .selected_display_profile
                .as_ref()
                .filter(|selected| remote_desktop_profile_supported(capabilities, selected))
                .filter(|selected| {
                    prefer_screenshot_only || *selected != REMOTE_DESKTOP_PROFILE_SCREENSHOT_ONLY
                })
                .cloned()
        })
}

fn choose_remote_desktop_normal_profile(
    capabilities: &RemoteDesktopCapabilities,
) -> Option<String> {
    choose_remote_desktop_profile(capabilities, false)
}

fn choose_remote_desktop_preferred_profile(
    capabilities: &RemoteDesktopCapabilities,
    preferred_profiles: &[String],
) -> Option<String> {
    preferred_profiles
        .iter()
        .map(|profile| profile.trim())
        .filter(|profile| !profile.is_empty())
        .filter(|profile| *profile != REMOTE_DESKTOP_PROFILE_SCREENSHOT_ONLY)
        .find(|profile| remote_desktop_profile_supported(capabilities, profile))
        .map(ToOwned::to_owned)
}

fn choose_remote_desktop_headless_profile(
    capabilities: &RemoteDesktopCapabilities,
) -> Result<Option<String>, AppError> {
    if !remote_desktop_profile_supported(capabilities, REMOTE_DESKTOP_PROFILE_SCREENSHOT_ONLY) {
        return Err(AppError::service_unavailable(
            "Headless screenshot capture is not supported by this agent",
        ));
    }
    Ok(choose_remote_desktop_profile(capabilities, true))
}

fn choose_remote_desktop_legacy_fallback_profile(
    capabilities: &RemoteDesktopCapabilities,
) -> Option<String> {
    [
        REMOTE_DESKTOP_PROFILE_LEGACY,
        REMOTE_DESKTOP_PROFILE_MODERN_CPU,
    ]
    .iter()
    .find(|candidate| {
        capabilities
            .display_profiles
            .iter()
            .any(|profile| profile.id == **candidate)
    })
    .map(|profile| (*profile).to_string())
}

fn remote_desktop_h264_startup_failure(reason: &str) -> bool {
    matches!(reason, "h264_encoder_unavailable" | "h264_encode_failed")
}

async fn start_remote_desktop_session(
    state: &AppState,
    agent_id: &str,
    api_base: Option<String>,
    deep_link_mode: &'static str,
    session_mode: SessionTransportMode,
    viewer_identity: Option<ViewerIdentity>,
    display_profile_preference: Option<&[String]>,
    hide_cursor: bool,
) -> Result<ConnectResponse, AppError> {
    let agent = {
        let agents = state.agents.read().await;
        agents.get(agent_id).cloned()
    };

    let Some(agent) = agent else {
        return Err(AppError::not_found("agent not connected"));
    };

    let session_id = Uuid::new_v4().to_string();
    let token = Uuid::new_v4().to_string();
    info!(
        agent_id = %agent_id,
        session_id = %session_id,
        "connect_device started"
    );

    let request_id = Uuid::new_v4().to_string();
    let (response_tx, response_rx) = oneshot::channel();
    {
        let mut requests = state.capability_requests.write().await;
        requests.insert(
            request_id.clone(),
            PendingCapabilityRequest {
                response_tx,
                created_at: Instant::now(),
            },
        );
    }

    let payload = SessionCapabilitiesRequest {
        request_id: request_id.clone(),
    };
    let envelope = OutgoingEnvelope {
        message_type: "session_capabilities_request",
        data: payload,
    };
    let message =
        serde_json::to_string(&envelope).context("serialize session_capabilities_request")?;
    agent
        .sender
        .send(Message::Text(message))
        .await
        .map_err(|_| AppError::bad_request("agent unavailable"))?;
    info!(request_id = %request_id, "session_capabilities_request sent");

    let mut capabilities = match timeout(Duration::from_secs(10), response_rx).await {
        Ok(Ok(capabilities)) => capabilities,
        _ => {
            let mut requests = state.capability_requests.write().await;
            requests.remove(&request_id);
            return Err(AppError::timeout("capabilities request timed out"));
        }
    };
    let prefer_screenshot_only = session_mode == SessionTransportMode::HeadlessRemoteDesktop;
    let mut selected_display_profile = if prefer_screenshot_only {
        choose_remote_desktop_headless_profile(&capabilities)?
    } else if let Some(preferred_profiles) = display_profile_preference {
        choose_remote_desktop_preferred_profile(&capabilities, preferred_profiles)
            .or_else(|| choose_remote_desktop_normal_profile(&capabilities))
    } else {
        choose_remote_desktop_normal_profile(&capabilities)
    };
    capabilities.selected_display_profile = selected_display_profile.clone();
    if !capabilities.features.remote_desktop {
        return Err(app_error_for_unsupported(
            "remote_desktop",
            capabilities.platform,
        ));
    }
    info!(session_id = %session_id, "session_capabilities_response received");

    let rcgen::CertifiedKey { cert, signing_key } =
        generate_simple_self_signed(vec!["rmm.local".to_string()])
            .context("generate session certificate")?;
    let psk_cert_pem = cert.pem();
    let psk_key_pem = signing_key.serialize_pem();
    info!(session_id = %session_id, "session certificate generated");

    let mut e2e_key = vec![0u8; 32];
    OsRng.fill_bytes(&mut e2e_key);
    let e2e_key_b64 = BASE64_STANDARD.encode(&e2e_key);
    let relay_url = state.config.relay_url.clone();

    let agent_reflex = if prefer_screenshot_only {
        let tunnel_prepare = TunnelPreparePayload {
            session_id: session_id.clone(),
            psk_cert_pem: psk_cert_pem.clone(),
            psk_key_pem: psk_key_pem.clone(),
            relay_url: relay_url.clone(),
            e2e_key: relay_url.as_ref().map(|_| e2e_key_b64.clone()),
            mode: session_mode,
            selected_display_profile: selected_display_profile.clone(),
            hide_cursor,
            viewer_session_token: None,
            parent_desktop_session_id: None,
        };
        let envelope = OutgoingEnvelope {
            message_type: "tunnel_prepare",
            data: tunnel_prepare,
        };
        let message = serde_json::to_string(&envelope).context("serialize tunnel_prepare")?;
        agent
            .sender
            .send(Message::Text(message))
            .await
            .map_err(|_| AppError::bad_request("agent unavailable"))?;
        info!(
            session_id = %session_id,
            selected_display_profile = ?selected_display_profile,
            "headless relay-only tunnel_prepare sent"
        );
        None
    } else {
        let mut h264_fallback_attempted = false;
        loop {
            let (reflex_tx, reflex_rx) = oneshot::channel();
            {
                let mut requests = state.quic_reflex_requests.write().await;
                requests.insert(
                    session_id.clone(),
                    PendingQuicReflex {
                        response_tx: reflex_tx,
                        created_at: Instant::now(),
                    },
                );
            }

            let tunnel_prepare = TunnelPreparePayload {
                session_id: session_id.clone(),
                psk_cert_pem: psk_cert_pem.clone(),
                psk_key_pem: psk_key_pem.clone(),
                relay_url: relay_url.clone(),
                e2e_key: relay_url.as_ref().map(|_| e2e_key_b64.clone()),
                mode: session_mode,
                selected_display_profile: selected_display_profile.clone(),
                hide_cursor,
                viewer_session_token: None,
                parent_desktop_session_id: None,
            };
            let envelope = OutgoingEnvelope {
                message_type: "tunnel_prepare",
                data: tunnel_prepare,
            };
            let message = serde_json::to_string(&envelope).context("serialize tunnel_prepare")?;
            agent
                .sender
                .send(Message::Text(message))
                .await
                .map_err(|_| AppError::bad_request("agent unavailable"))?;
            info!(
                session_id = %session_id,
                selected_display_profile = ?selected_display_profile,
                "tunnel_prepare sent"
            );

            // Wait up to 15 s for quic_reflex OR remote_desktop_unavailable from the
            // agent. The increased timeout covers the display readiness check
            // (DXGI probe + display topology refresh + retry ~= up to 12 s).
            match timeout(Duration::from_secs(15), reflex_rx).await {
                Ok(Ok(QuicReflexResult::Success(reflex))) => {
                    info!(session_id = %session_id, "quic_reflex received");
                    break Some(reflex);
                }
                Ok(Ok(QuicReflexResult::DisplayUnavailable { reason, message })) => {
                    let mut requests = state.quic_reflex_requests.write().await;
                    requests.remove(&session_id);
                    if !h264_fallback_attempted && remote_desktop_h264_startup_failure(&reason) {
                        if let Some(fallback_profile) =
                            choose_remote_desktop_legacy_fallback_profile(&capabilities)
                        {
                            h264_fallback_attempted = true;
                            warn!(
                                session_id = %session_id,
                                reason = %reason,
                                previous_display_profile = ?selected_display_profile,
                                fallback_display_profile = %fallback_profile,
                                "agent H.264 startup failed; retrying remote desktop with legacy profile"
                            );
                            selected_display_profile = Some(fallback_profile);
                            capabilities.selected_display_profile =
                                selected_display_profile.clone();
                            continue;
                        }
                    }
                    let detail = message.unwrap_or_else(|| reason.clone());
                    warn!(session_id = %session_id, reason = %reason, "agent reported display unavailable");
                    return Err(AppError::service_unavailable(&format!(
                        "Remote desktop unavailable: {detail}"
                    )));
                }
                _ => {
                    let mut requests = state.quic_reflex_requests.write().await;
                    requests.remove(&session_id);
                    info!(session_id = %session_id, "quic_reflex timed out (e.g. UDP/STUN blocked); session created with relay only");
                    break None;
                }
            }
        }
    };

    let agent_local_addrs = if !agent.local_addrs.is_empty() {
        agent.local_addrs.clone()
    } else {
        agent
            .host
            .clone()
            .map(|ip| vec![LocalAddr { ip, prefix: 24 }])
            .unwrap_or_default()
    };

    {
        let mut sessions = state.remote_desktop_sessions.write().await;
        let viewer_user_id = viewer_identity
            .as_ref()
            .map(|identity| identity.user_id.clone());
        let viewer_user_email = viewer_identity
            .as_ref()
            .and_then(|identity| identity.user_email.clone());
        sessions.insert(
            session_id.clone(),
            RemoteDesktopSession {
                token: token.clone(),
                mode: session_mode,
                hide_cursor,
                viewer_user_id,
                viewer_user_email,
                capabilities: capabilities.clone(),
                agent_reflex: agent_reflex.clone(),
                viewer_reflex: None,
                agent_host: agent.host.clone(),
                agent_local_addrs,
                psk_cert_pem: psk_cert_pem.clone(),
                e2e_key,
                relay_url: relay_url.clone(),
                agent_id: agent_id.to_string(),
                agent_hostname: agent.hostname.clone(),
                agent_os: agent.os.clone(),
                agent_version: agent.version.clone(),
                created_at: Instant::now(),
                attached_at: None,
                viewer_connected_at: None,
                viewer_last_heartbeat_at: None,
            },
        );
    }

    let url = build_desktop_connect_url_with_mode(
        &session_id,
        &token,
        agent_id,
        api_base.as_deref(),
        deep_link_mode,
    );
    info!(session_id = %session_id, "remote desktop session ready");

    let device_scope = fetch_device_scope(state, agent_id).await?;
    log_audit_event(
        state,
        Some(&device_scope.organization_id),
        device_scope.customer_id.as_deref(),
        device_scope.site_id.as_deref(),
        Some(agent_id),
        viewer_identity.as_ref(),
        "remote_desktop.start",
        "rmm_device",
        Some(agent_id),
        agent.hostname.as_deref(),
        "success",
        Some(&session_id),
        serde_json::json!({
            "deepLinkMode": deep_link_mode,
            "transports": capabilities.transports,
            "selectedDisplayProfile": capabilities.selected_display_profile,
            "hideCursor": hide_cursor,
        }),
    )
    .await?;

    Ok(ConnectResponse { url, session_id })
}

async fn start_file_transfer_session(
    state: &AppState,
    agent_id: &str,
    api_base: Option<String>,
    viewer_identity: Option<ViewerIdentity>,
) -> Result<ConnectResponse, AppError> {
    let agent = {
        let agents = state.agents.read().await;
        agents.get(agent_id).cloned()
    };

    let Some(agent) = agent else {
        return Err(AppError::not_found("agent not connected"));
    };

    let session_id = Uuid::new_v4().to_string();
    let token = Uuid::new_v4().to_string();
    info!(
        agent_id = %agent_id,
        session_id = %session_id,
        "connect_file_transfer started"
    );

    let request_id = Uuid::new_v4().to_string();
    let (response_tx, response_rx) = oneshot::channel();
    {
        let mut requests = state.capability_requests.write().await;
        requests.insert(
            request_id.clone(),
            PendingCapabilityRequest {
                response_tx,
                created_at: Instant::now(),
            },
        );
    }

    let payload = SessionCapabilitiesRequest {
        request_id: request_id.clone(),
    };
    let envelope = OutgoingEnvelope {
        message_type: "session_capabilities_request",
        data: payload,
    };
    let message =
        serde_json::to_string(&envelope).context("serialize session_capabilities_request")?;
    agent
        .sender
        .send(Message::Text(message))
        .await
        .map_err(|_| AppError::bad_request("agent unavailable"))?;
    info!(request_id = %request_id, "session_capabilities_request sent for file transfer");

    let capabilities = match timeout(Duration::from_secs(10), response_rx).await {
        Ok(Ok(capabilities)) => capabilities,
        _ => {
            let mut requests = state.capability_requests.write().await;
            requests.remove(&request_id);
            return Err(AppError::timeout("capabilities request timed out"));
        }
    };
    if !capabilities.features.file_transfer {
        return Err(app_error_for_unsupported(
            "file_transfer",
            capabilities.platform,
        ));
    }
    info!(session_id = %session_id, "session_capabilities_response received for file transfer");

    let rcgen::CertifiedKey { cert, signing_key } =
        generate_simple_self_signed(vec!["rmm.local".to_string()])
            .context("generate session certificate")?;
    let psk_cert_pem = cert.pem();
    let psk_key_pem = signing_key.serialize_pem();
    info!(session_id = %session_id, "file transfer session certificate generated");

    let mut e2e_key = vec![0u8; 32];
    OsRng.fill_bytes(&mut e2e_key);
    let e2e_key_b64 = BASE64_STANDARD.encode(&e2e_key);
    let relay_url = state.config.relay_url.clone();

    let (reflex_tx, reflex_rx) = oneshot::channel();
    {
        let mut requests = state.quic_reflex_requests.write().await;
        requests.insert(
            session_id.clone(),
            PendingQuicReflex {
                response_tx: reflex_tx,
                created_at: Instant::now(),
            },
        );
    }

    let tunnel_prepare = TunnelPreparePayload {
        session_id: session_id.clone(),
        psk_cert_pem: psk_cert_pem.clone(),
        psk_key_pem,
        relay_url: relay_url.clone(),
        e2e_key: relay_url.as_ref().map(|_| e2e_key_b64.clone()),
        mode: SessionTransportMode::FileTransfer,
        selected_display_profile: None,
        hide_cursor: false,
        viewer_session_token: None,
        parent_desktop_session_id: None,
    };
    let envelope = OutgoingEnvelope {
        message_type: "tunnel_prepare",
        data: tunnel_prepare,
    };
    let message = serde_json::to_string(&envelope).context("serialize tunnel_prepare")?;
    agent
        .sender
        .send(Message::Text(message))
        .await
        .map_err(|_| AppError::bad_request("agent unavailable"))?;
    info!(session_id = %session_id, "file transfer tunnel_prepare sent");

    let agent_reflex = match timeout(Duration::from_secs(10), reflex_rx).await {
        Ok(Ok(QuicReflexResult::Success(reflex))) => {
            info!(session_id = %session_id, "file transfer quic_reflex received");
            Some(reflex)
        }
        Ok(Ok(QuicReflexResult::DisplayUnavailable { .. })) => {
            let mut requests = state.quic_reflex_requests.write().await;
            requests.remove(&session_id);
            None
        }
        _ => {
            let mut requests = state.quic_reflex_requests.write().await;
            requests.remove(&session_id);
            info!(
                session_id = %session_id,
                "file transfer quic_reflex timed out (relay-only)"
            );
            None
        }
    };

    let agent_local_addrs = if !agent.local_addrs.is_empty() {
        agent.local_addrs.clone()
    } else {
        agent
            .host
            .clone()
            .map(|ip| vec![LocalAddr { ip, prefix: 24 }])
            .unwrap_or_default()
    };

    {
        let mut sessions = state.file_transfer_sessions.write().await;
        let viewer_user_id = viewer_identity
            .as_ref()
            .map(|identity| identity.user_id.clone());
        let viewer_user_email = viewer_identity
            .as_ref()
            .and_then(|identity| identity.user_email.clone());
        sessions.insert(
            session_id.clone(),
            FileTransferSession {
                token: token.clone(),
                viewer_user_id,
                viewer_user_email,
                transports: capabilities.transports.clone(),
                agent_reflex: agent_reflex.clone(),
                viewer_reflex: None,
                agent_host: agent.host.clone(),
                agent_local_addrs,
                platform: capabilities.platform,
                features: capabilities.features.clone(),
                psk_cert_pem: psk_cert_pem.clone(),
                e2e_key,
                relay_url: relay_url.clone(),
                agent_id: agent_id.to_string(),
                created_at: Instant::now(),
                attached_at: None,
                viewer_connected_at: None,
                viewer_last_heartbeat_at: None,
            },
        );
    }

    let url = build_file_transfer_connect_url(&session_id, &token, agent_id, api_base.as_deref());
    info!(session_id = %session_id, "file transfer session ready");

    let device_scope = fetch_device_scope(state, agent_id).await?;
    log_audit_event(
        state,
        Some(&device_scope.organization_id),
        device_scope.customer_id.as_deref(),
        device_scope.site_id.as_deref(),
        Some(agent_id),
        viewer_identity.as_ref(),
        "file_transfer.start",
        "rmm_device",
        Some(agent_id),
        agent.hostname.as_deref(),
        "success",
        Some(&session_id),
        serde_json::json!({
            "transports": capabilities.transports,
        }),
    )
    .await?;

    Ok(ConnectResponse { url, session_id })
}

async fn start_remote_registry_session(
    state: &AppState,
    agent_id: &str,
    api_base: Option<String>,
    viewer_identity: Option<ViewerIdentity>,
) -> Result<ConnectResponse, AppError> {
    let agent = {
        let agents = state.agents.read().await;
        agents.get(agent_id).cloned()
    };

    let Some(agent) = agent else {
        return Err(AppError::not_found("agent not connected"));
    };

    let session_id = Uuid::new_v4().to_string();
    let token = Uuid::new_v4().to_string();
    info!(
        agent_id = %agent_id,
        session_id = %session_id,
        "connect_registry started"
    );

    let request_id = Uuid::new_v4().to_string();
    let (response_tx, response_rx) = oneshot::channel();
    {
        let mut requests = state.capability_requests.write().await;
        requests.insert(
            request_id.clone(),
            PendingCapabilityRequest {
                response_tx,
                created_at: Instant::now(),
            },
        );
    }

    let payload = SessionCapabilitiesRequest {
        request_id: request_id.clone(),
    };
    let envelope = OutgoingEnvelope {
        message_type: "session_capabilities_request",
        data: payload,
    };
    let message =
        serde_json::to_string(&envelope).context("serialize session_capabilities_request")?;
    agent
        .sender
        .send(Message::Text(message))
        .await
        .map_err(|_| AppError::bad_request("agent unavailable"))?;
    info!(request_id = %request_id, "session_capabilities_request sent for registry");

    let capabilities = match timeout(Duration::from_secs(10), response_rx).await {
        Ok(Ok(capabilities)) => capabilities,
        _ => {
            let mut requests = state.capability_requests.write().await;
            requests.remove(&request_id);
            return Err(AppError::timeout("capabilities request timed out"));
        }
    };
    if !capabilities.features.remote_registry {
        return Err(app_error_for_unsupported(
            "remote_registry",
            capabilities.platform,
        ));
    }
    info!(session_id = %session_id, "session_capabilities_response received for registry");

    let rcgen::CertifiedKey { cert, signing_key } =
        generate_simple_self_signed(vec!["rmm.local".to_string()])
            .context("generate registry session certificate")?;
    let psk_cert_pem = cert.pem();
    let psk_key_pem = signing_key.serialize_pem();

    let mut e2e_key = vec![0u8; 32];
    OsRng.fill_bytes(&mut e2e_key);
    let e2e_key_b64 = BASE64_STANDARD.encode(&e2e_key);
    let relay_url = state.config.relay_url.clone();

    let (reflex_tx, reflex_rx) = oneshot::channel();
    {
        let mut requests = state.quic_reflex_requests.write().await;
        requests.insert(
            session_id.clone(),
            PendingQuicReflex {
                response_tx: reflex_tx,
                created_at: Instant::now(),
            },
        );
    }

    let tunnel_prepare = TunnelPreparePayload {
        session_id: session_id.clone(),
        psk_cert_pem: psk_cert_pem.clone(),
        psk_key_pem,
        relay_url: relay_url.clone(),
        e2e_key: relay_url.as_ref().map(|_| e2e_key_b64.clone()),
        mode: SessionTransportMode::RemoteRegistry,
        selected_display_profile: None,
        hide_cursor: false,
        viewer_session_token: None,
        parent_desktop_session_id: None,
    };
    let envelope = OutgoingEnvelope {
        message_type: "tunnel_prepare",
        data: tunnel_prepare,
    };
    let message = serde_json::to_string(&envelope).context("serialize tunnel_prepare")?;
    agent
        .sender
        .send(Message::Text(message))
        .await
        .map_err(|_| AppError::bad_request("agent unavailable"))?;
    info!(session_id = %session_id, "registry tunnel_prepare sent");

    let agent_reflex = match timeout(Duration::from_secs(10), reflex_rx).await {
        Ok(Ok(QuicReflexResult::Success(reflex))) => {
            info!(session_id = %session_id, "registry quic_reflex received");
            Some(reflex)
        }
        Ok(Ok(QuicReflexResult::DisplayUnavailable { .. })) => {
            let mut requests = state.quic_reflex_requests.write().await;
            requests.remove(&session_id);
            None
        }
        _ => {
            let mut requests = state.quic_reflex_requests.write().await;
            requests.remove(&session_id);
            info!(session_id = %session_id, "registry quic_reflex timed out (relay-only)");
            None
        }
    };

    let agent_local_addrs = if !agent.local_addrs.is_empty() {
        agent.local_addrs.clone()
    } else {
        agent
            .host
            .clone()
            .map(|ip| vec![LocalAddr { ip, prefix: 24 }])
            .unwrap_or_default()
    };

    {
        let mut sessions = state.remote_registry_sessions.write().await;
        let viewer_user_id = viewer_identity
            .as_ref()
            .map(|identity| identity.user_id.clone());
        let viewer_user_email = viewer_identity
            .as_ref()
            .and_then(|identity| identity.user_email.clone());
        sessions.insert(
            session_id.clone(),
            RemoteRegistrySession {
                token: token.clone(),
                viewer_user_id,
                viewer_user_email,
                transports: capabilities.transports.clone(),
                agent_reflex: agent_reflex.clone(),
                viewer_reflex: None,
                agent_host: agent.host.clone(),
                agent_local_addrs,
                platform: capabilities.platform,
                features: capabilities.features.clone(),
                psk_cert_pem: psk_cert_pem.clone(),
                e2e_key,
                relay_url: relay_url.clone(),
                agent_id: agent_id.to_string(),
                created_at: Instant::now(),
                attached_at: None,
                viewer_connected_at: None,
                viewer_last_heartbeat_at: None,
            },
        );
    }

    let url = build_registry_connect_url(&session_id, &token, agent_id, api_base.as_deref());
    info!(session_id = %session_id, "remote registry session ready");

    let device_scope = fetch_device_scope(state, agent_id).await?;
    log_audit_event(
        state,
        Some(&device_scope.organization_id),
        device_scope.customer_id.as_deref(),
        device_scope.site_id.as_deref(),
        Some(agent_id),
        viewer_identity.as_ref(),
        "registry.start",
        "rmm_device",
        Some(agent_id),
        agent.hostname.as_deref(),
        "success",
        Some(&session_id),
        serde_json::json!({
            "transports": capabilities.transports,
        }),
    )
    .await?;

    Ok(ConnectResponse { url, session_id })
}

async fn start_chat_session(
    state: &AppState,
    agent_id: &str,
    api_base: Option<String>,
    viewer_identity: Option<ViewerIdentity>,
    parent_desktop_session_id: Option<String>,
) -> Result<ConnectResponse, AppError> {
    let agent = {
        let agents = state.agents.read().await;
        agents.get(agent_id).cloned()
    };

    let Some(agent) = agent else {
        return Err(AppError::not_found("agent not connected"));
    };

    let session_id = Uuid::new_v4().to_string();
    let token = Uuid::new_v4().to_string();
    info!(
        agent_id = %agent_id,
        session_id = %session_id,
        parent_desktop_session_id = ?parent_desktop_session_id,
        "connect_chat started"
    );

    let request_id = Uuid::new_v4().to_string();
    let (response_tx, response_rx) = oneshot::channel();
    {
        let mut requests = state.capability_requests.write().await;
        requests.insert(
            request_id.clone(),
            PendingCapabilityRequest {
                response_tx,
                created_at: Instant::now(),
            },
        );
    }

    let payload = SessionCapabilitiesRequest {
        request_id: request_id.clone(),
    };
    let envelope = OutgoingEnvelope {
        message_type: "session_capabilities_request",
        data: payload,
    };
    let message =
        serde_json::to_string(&envelope).context("serialize session_capabilities_request")?;
    agent
        .sender
        .send(Message::Text(message))
        .await
        .map_err(|_| AppError::bad_request("agent unavailable"))?;
    info!(request_id = %request_id, "session_capabilities_request sent for chat");

    let capabilities = match timeout(Duration::from_secs(10), response_rx).await {
        Ok(Ok(capabilities)) => capabilities,
        _ => {
            let mut requests = state.capability_requests.write().await;
            requests.remove(&request_id);
            return Err(AppError::timeout("capabilities request timed out"));
        }
    };
    if !capabilities.features.chat {
        return Err(app_error_for_unsupported("chat", capabilities.platform));
    }
    info!(session_id = %session_id, "session_capabilities_response received for chat");

    let rcgen::CertifiedKey { cert, signing_key } =
        generate_simple_self_signed(vec!["rmm.local".to_string()])
            .context("generate chat session certificate")?;
    let psk_cert_pem = cert.pem();
    let psk_key_pem = signing_key.serialize_pem();
    info!(session_id = %session_id, "chat session certificate generated");

    let mut e2e_key = vec![0u8; 32];
    OsRng.fill_bytes(&mut e2e_key);
    let e2e_key_b64 = BASE64_STANDARD.encode(&e2e_key);
    let relay_url = state.config.relay_url.clone();

    let (reflex_tx, reflex_rx) = oneshot::channel();
    {
        let mut requests = state.quic_reflex_requests.write().await;
        requests.insert(
            session_id.clone(),
            PendingQuicReflex {
                response_tx: reflex_tx,
                created_at: Instant::now(),
            },
        );
    }

    let tunnel_prepare = TunnelPreparePayload {
        session_id: session_id.clone(),
        psk_cert_pem: psk_cert_pem.clone(),
        psk_key_pem,
        relay_url: relay_url.clone(),
        e2e_key: relay_url.as_ref().map(|_| e2e_key_b64.clone()),
        mode: SessionTransportMode::Chat,
        selected_display_profile: None,
        hide_cursor: false,
        viewer_session_token: Some(token.clone()),
        parent_desktop_session_id: parent_desktop_session_id.clone(),
    };
    let envelope = OutgoingEnvelope {
        message_type: "tunnel_prepare",
        data: tunnel_prepare,
    };
    let message = serde_json::to_string(&envelope).context("serialize tunnel_prepare")?;
    agent
        .sender
        .send(Message::Text(message))
        .await
        .map_err(|_| AppError::bad_request("agent unavailable"))?;
    info!(session_id = %session_id, "chat tunnel_prepare sent");

    let agent_reflex = match timeout(Duration::from_secs(10), reflex_rx).await {
        Ok(Ok(QuicReflexResult::Success(reflex))) => {
            info!(session_id = %session_id, "chat quic_reflex received");
            Some(reflex)
        }
        Ok(Ok(QuicReflexResult::DisplayUnavailable { .. })) => {
            let mut requests = state.quic_reflex_requests.write().await;
            requests.remove(&session_id);
            None
        }
        _ => {
            let mut requests = state.quic_reflex_requests.write().await;
            requests.remove(&session_id);
            info!(
                session_id = %session_id,
                "chat quic_reflex timed out (relay-only)"
            );
            None
        }
    };

    let agent_local_addrs = if !agent.local_addrs.is_empty() {
        agent.local_addrs.clone()
    } else {
        agent
            .host
            .clone()
            .map(|ip| vec![LocalAddr { ip, prefix: 24 }])
            .unwrap_or_default()
    };

    {
        let mut sessions = state.chat_sessions.write().await;
        let viewer_user_id = viewer_identity
            .as_ref()
            .map(|identity| identity.user_id.clone());
        let viewer_user_email = viewer_identity
            .as_ref()
            .and_then(|identity| identity.user_email.clone());
        sessions.insert(
            session_id.clone(),
            ChatSession {
                token: token.clone(),
                viewer_user_id,
                viewer_user_email,
                parent_desktop_session_id: parent_desktop_session_id.clone(),
                transports: capabilities.transports.clone(),
                agent_reflex: agent_reflex.clone(),
                viewer_reflex: None,
                agent_host: agent.host.clone(),
                agent_local_addrs,
                platform: capabilities.platform,
                features: capabilities.features.clone(),
                psk_cert_pem: psk_cert_pem.clone(),
                e2e_key,
                relay_url: relay_url.clone(),
                agent_id: agent_id.to_string(),
                created_at: Instant::now(),
                attached_at: None,
                viewer_connected_at: None,
                viewer_last_heartbeat_at: None,
            },
        );
    }

    let url = build_chat_connect_url(&session_id, &token, agent_id, api_base.as_deref());
    info!(session_id = %session_id, "chat session ready");

    let device_scope = fetch_device_scope(state, agent_id).await?;
    log_audit_event(
        state,
        Some(&device_scope.organization_id),
        device_scope.customer_id.as_deref(),
        device_scope.site_id.as_deref(),
        Some(agent_id),
        viewer_identity.as_ref(),
        "chat.start",
        "rmm_device",
        Some(agent_id),
        agent.hostname.as_deref(),
        "success",
        Some(&session_id),
        serde_json::json!({
            "parentDesktopSessionId": parent_desktop_session_id,
            "transports": capabilities.transports,
        }),
    )
    .await?;

    Ok(ConnectResponse { url, session_id })
}

struct ShellSessionStartOptions {
    api_base: Option<String>,
    run_as: ShellRunAs,
    target_session_id: Option<u32>,
    viewer_identity: Option<ViewerIdentity>,
    audit_scope: DeviceScope,
    audit_action: &'static str,
    audit_metadata: Value,
    reuse_session: Option<(String, String)>,
}

async fn start_shell_session(
    state: &AppState,
    agent_id: &str,
    options: ShellSessionStartOptions,
) -> Result<ConnectResponse, AppError> {
    let agent = {
        let agents = state.agents.read().await;
        agents.get(agent_id).cloned()
    };

    let Some(agent) = agent else {
        return Err(AppError::not_found("agent not connected"));
    };

    let agent_features = features_for_agent(&agent);
    if !agent_features.system_shell {
        if agent.platform == AgentPlatform::Macos && !agent.is_admin {
            return Err(AppError::bad_request(
                system_shell_requires_elevation_message(agent.platform),
            ));
        }
        return Err(app_error_for_unsupported("system_shell", agent.platform));
    }

    if options.run_as == ShellRunAs::User && agent.platform != AgentPlatform::Windows {
        return Err(AppError::bad_request(user_shell_unsupported_message(
            agent.platform,
        )));
    }
    let supports_system_shell = agent.is_admin;
    if options.run_as == ShellRunAs::System && !supports_system_shell {
        return Err(AppError::bad_request(
            system_shell_requires_elevation_message(agent.platform),
        ));
    }

    let relay_url = state.config.relay_url.clone();
    let mut e2e_key = vec![0u8; 32];
    OsRng.fill_bytes(&mut e2e_key);
    let e2e_key_b64 = BASE64_URL_SAFE_NO_PAD.encode(&e2e_key);
    let shell_e2e_key = relay_url.as_ref().map(|_| e2e_key_b64.clone());

    let rcgen::CertifiedKey { cert, signing_key } =
        generate_simple_self_signed(vec!["rmm.local".to_string()])
            .context("generate shell session certificate")?;
    let psk_cert_pem = cert.pem();
    let psk_key_pem = signing_key.serialize_pem();

    let (session_id, token) = options
        .reuse_session
        .clone()
        .unwrap_or_else(|| (Uuid::new_v4().to_string(), Uuid::new_v4().to_string()));
    let (response_tx, response_rx) = oneshot::channel();

    {
        let mut sessions = state.shell_sessions.write().await;
        sessions.insert(
            session_id.clone(),
            PendingShellSession {
                response_tx,
                created_at: Instant::now(),
            },
        );
    }

    let payload = ShellStartPayload {
        session_id: session_id.clone(),
        token: token.clone(),
        run_as: options.run_as,
        target_session_id: options.target_session_id,
        relay_url: relay_url.clone(),
        e2e_key: shell_e2e_key.clone(),
        psk_cert_pem: Some(psk_cert_pem.clone()),
        psk_key_pem: Some(psk_key_pem),
    };
    let envelope = OutgoingEnvelope {
        message_type: "shell_start",
        data: payload,
    };
    let message = serde_json::to_string(&envelope).context("serialize shell_start")?;
    agent
        .sender
        .send(Message::Text(message))
        .await
        .map_err(|_| AppError::bad_request("agent unavailable"))?;

    let offer = match timeout(Duration::from_secs(15), response_rx).await {
        Ok(Ok(Ok(offer))) => offer,
        Ok(Ok(Err(error_message))) => {
            return Err(AppError::bad_request(&error_message));
        }
        Ok(Err(_)) => {
            let mut sessions = state.shell_sessions.write().await;
            sessions.remove(&session_id);
            return Err(AppError::bad_request("shell session aborted"));
        }
        _ => {
            let mut sessions = state.shell_sessions.write().await;
            sessions.remove(&session_id);
            return Err(AppError::timeout("shell session timed out"));
        }
    };

    let host = if offer.host.trim().is_empty() {
        agent.host.unwrap_or_else(|| "127.0.0.1".to_string())
    } else {
        offer.host.clone()
    };
    let url = build_connect_url(
        &host,
        offer.stream_port,
        &offer.session_id,
        &token,
        agent_id,
        options.api_base.as_deref(),
        Some("shell"),
        Some(options.run_as),
        options.target_session_id,
        supports_system_shell,
        relay_url.as_deref(),
        shell_e2e_key.as_deref(),
    );

    let mut transports = Vec::new();
    if offer.reflex.is_some() && !psk_cert_pem.is_empty() {
        transports.push("quic".to_string());
    }
    if relay_url.is_some() {
        transports.push("relay".to_string());
    }
    let audit_transports = transports.clone();

    {
        let mut sessions = state.active_shell_sessions.write().await;
        sessions.insert(
            session_id.clone(),
            ActiveShellSession {
                token: token.clone(),
                agent_id: agent_id.to_string(),
                viewer_user_id: options
                    .viewer_identity
                    .as_ref()
                    .map(|identity| identity.user_id.clone()),
                viewer_user_email: options
                    .viewer_identity
                    .as_ref()
                    .and_then(|identity| identity.user_email.clone()),
                transports,
                agent_reflex: offer.reflex,
                viewer_reflex: None,
                agent_host: Some(host.clone()),
                agent_local_addrs: offer.local_addrs,
                platform: agent.platform,
                features: agent_features.clone(),
                psk_cert_pem: Some(psk_cert_pem),
                e2e_key,
                relay_url: relay_url.clone(),
                created_at: Instant::now(),
                attached_at: Some(Instant::now()),
                viewer_connected_at: None,
                viewer_last_heartbeat_at: None,
            },
        );
    }

    let mut metadata = options.audit_metadata;
    if let Some(object) = metadata.as_object_mut() {
        object.insert("runAs".to_string(), serde_json::json!(options.run_as));
        object.insert(
            "targetSessionId".to_string(),
            serde_json::json!(options.target_session_id),
        );
        object.insert(
            "transports".to_string(),
            serde_json::json!(audit_transports),
        );
    }
    log_audit_event(
        state,
        Some(&options.audit_scope.organization_id),
        options.audit_scope.customer_id.as_deref(),
        options.audit_scope.site_id.as_deref(),
        Some(agent_id),
        options.viewer_identity.as_ref(),
        options.audit_action,
        "rmm_device",
        Some(agent_id),
        None,
        "success",
        Some(&session_id),
        metadata,
    )
    .await?;

    Ok(ConnectResponse { url, session_id })
}

async fn connect_shell(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    Query(query): Query<ShellConnectQuery>,
    headers: HeaderMap,
) -> Result<Json<ConnectResponse>, AppError> {
    enum ShellConnectAuth {
        Bearer {
            user_context: UserContext,
            device_scope: DeviceScope,
        },
        SessionToken {
            viewer_identity: Option<ViewerIdentity>,
        },
    }

    // Shell connect is called from:
    // - the web dashboard (bearer auth)
    // - the viewer app launched via deep link (session token auth; no bearer available)
    let auth = if extract_bearer(&headers).is_some() {
        let user_context = require_user_context(&state, &headers).await?;
        ensure_device_in_organization(&state, &agent_id, &user_context.organization_id).await?;
        let device_scope = fetch_device_scope(&state, &agent_id).await?;
        if device_scope.organization_id != user_context.organization_id {
            return Err(AppError::forbidden(
                "device does not belong to your organization",
            ));
        }
        ShellConnectAuth::Bearer {
            user_context,
            device_scope,
        }
    } else if let (Some(session_id), Some(token)) = (query.session.clone(), query.token.clone()) {
        let session = {
            let sessions = state.remote_desktop_sessions.read().await;
            sessions.get(&session_id).cloned()
        };
        let Some(session) = session else {
            return Err(AppError::not_found("session not found"));
        };
        if session.token != token {
            return Err(AppError::unauthorized("invalid session token"));
        }
        if session.agent_id != agent_id {
            return Err(AppError::forbidden("session does not match agent"));
        }
        ShellConnectAuth::SessionToken {
            viewer_identity: session
                .viewer_user_id
                .as_ref()
                .map(|user_id| ViewerIdentity {
                    user_id: user_id.clone(),
                    user_email: session.viewer_user_email.clone(),
                }),
        }
    } else {
        return Err(AppError::unauthorized("missing bearer token"));
    };

    let run_as = query.run_as.unwrap_or(ShellRunAs::System);
    let target_session_id = query.target_session_id;
    let api_base = extract_api_base(&headers);
    let viewer_identity = match &auth {
        ShellConnectAuth::Bearer { user_context, .. } => Some(ViewerIdentity {
            user_id: user_context.user_id.clone(),
            user_email: user_context.email.clone(),
        }),
        ShellConnectAuth::SessionToken { viewer_identity } => viewer_identity.clone(),
    };
    let audit_scope = match &auth {
        ShellConnectAuth::Bearer { device_scope, .. } => device_scope.clone(),
        ShellConnectAuth::SessionToken { .. } => fetch_device_scope(&state, &agent_id).await?,
    };

    let reuse_session = match (&auth, query.session.clone(), query.token.clone()) {
        (ShellConnectAuth::Bearer { .. }, Some(session_id), Some(token)) => {
            Some((session_id, token))
        }
        _ => None,
    };
    let response = start_shell_session(
        &state,
        &agent_id,
        ShellSessionStartOptions {
            api_base,
            run_as,
            target_session_id,
            viewer_identity,
            audit_scope,
            audit_action: "shell.start",
            audit_metadata: serde_json::json!({}),
            reuse_session,
        },
    )
    .await?;

    Ok(Json(response))
}

async fn connect_ai_runner_shell_session_internal(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<InternalAiRunnerConnectRequest>,
) -> Result<Json<ConnectResponse>, AppError> {
    require_internal_server_key(&state.config, &headers)?;
    let organization_id = body.organization_id.trim();
    if organization_id.is_empty() {
        return Err(AppError::bad_request("organizationId is required"));
    }
    let device_scope = fetch_device_scope(&state, &agent_id).await?;
    if device_scope.organization_id != organization_id {
        return Err(AppError::forbidden(
            "device does not belong to the requested organization",
        ));
    }
    let api_base = body
        .api_base_url
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .or_else(|| extract_api_base(&headers));
    let response = start_shell_session(
        &state,
        &agent_id,
        ShellSessionStartOptions {
            api_base,
            run_as: body.run_as.unwrap_or(ShellRunAs::System),
            target_session_id: body.target_session_id,
            viewer_identity: None,
            audit_scope: device_scope.clone(),
            audit_action: "ai_runner.shell_session_lease",
            audit_metadata: serde_json::json!({
                "jobId": body.job_id,
            }),
            reuse_session: None,
        },
    )
    .await?;

    Ok(Json(response))
}
async fn get_viewer_session_status(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<ViewerSessionStatusResponse>, AppError> {
    let user_context = require_user_context(&state, &headers).await?;
    let snapshot = find_viewer_session_snapshot(&state, &session_id)
        .await
        .ok_or_else(|| AppError::not_found("viewer session not found"))?;
    ensure_device_in_organization(&state, &snapshot.agent_id, &user_context.organization_id)
        .await?;
    let device_scope = fetch_device_scope(&state, &snapshot.agent_id).await?;
    if device_scope.organization_id != user_context.organization_id {
        return Err(AppError::forbidden(
            "device does not belong to your organization",
        ));
    }
    Ok(Json(viewer_status_response_from_snapshot(snapshot)))
}

async fn list_viewer_connections(
    State(state): State<AppState>,
    Query(query): Query<ViewerConnectionsQuery>,
    headers: HeaderMap,
) -> Result<Json<Vec<ViewerConnectionSummary>>, AppError> {
    let user_context = require_user_context(&state, &headers).await?;
    if let Some(agent_id) = query.agent_id.as_deref() {
        ensure_device_in_organization(&state, agent_id, &user_context.organization_id).await?;
        let device_scope = fetch_device_scope(&state, agent_id).await?;
        if device_scope.organization_id != user_context.organization_id {
            return Err(AppError::forbidden(
                "device does not belong to your organization",
            ));
        }
    }

    let mut snapshots = Vec::new();
    {
        let sessions = state.remote_desktop_sessions.read().await;
        for (session_id, session) in sessions.iter() {
            if !is_presence_live(session.viewer_last_heartbeat_at) {
                continue;
            }
            if query
                .agent_id
                .as_deref()
                .is_some_and(|agent_id| agent_id != session.agent_id)
            {
                continue;
            }
            snapshots.push(ViewerSessionSnapshot {
                session_id: session_id.clone(),
                kind: ViewerSessionKind::RemoteDesktop,
                agent_id: session.agent_id.clone(),
                user_id: session.viewer_user_id.clone(),
                user_email: session.viewer_user_email.clone(),
                attached_at: session.attached_at,
                viewer_connected_at: session.viewer_connected_at,
                viewer_last_heartbeat_at: session.viewer_last_heartbeat_at,
            });
        }
    }
    {
        let sessions = state.active_shell_sessions.read().await;
        for (session_id, session) in sessions.iter() {
            if !is_presence_live(session.viewer_last_heartbeat_at) {
                continue;
            }
            if query
                .agent_id
                .as_deref()
                .is_some_and(|agent_id| agent_id != session.agent_id)
            {
                continue;
            }
            snapshots.push(ViewerSessionSnapshot {
                session_id: session_id.clone(),
                kind: ViewerSessionKind::Shell,
                agent_id: session.agent_id.clone(),
                user_id: session.viewer_user_id.clone(),
                user_email: session.viewer_user_email.clone(),
                attached_at: session.attached_at,
                viewer_connected_at: session.viewer_connected_at,
                viewer_last_heartbeat_at: session.viewer_last_heartbeat_at,
            });
        }
    }
    {
        let sessions = state.file_transfer_sessions.read().await;
        for (session_id, session) in sessions.iter() {
            if !is_presence_live(session.viewer_last_heartbeat_at) {
                continue;
            }
            if query
                .agent_id
                .as_deref()
                .is_some_and(|agent_id| agent_id != session.agent_id)
            {
                continue;
            }
            snapshots.push(ViewerSessionSnapshot {
                session_id: session_id.clone(),
                kind: ViewerSessionKind::FileTransfer,
                agent_id: session.agent_id.clone(),
                user_id: session.viewer_user_id.clone(),
                user_email: session.viewer_user_email.clone(),
                attached_at: session.attached_at,
                viewer_connected_at: session.viewer_connected_at,
                viewer_last_heartbeat_at: session.viewer_last_heartbeat_at,
            });
        }
    }
    {
        let sessions = state.remote_registry_sessions.read().await;
        for (session_id, session) in sessions.iter() {
            if !is_presence_live(session.viewer_last_heartbeat_at) {
                continue;
            }
            if query
                .agent_id
                .as_deref()
                .is_some_and(|agent_id| agent_id != session.agent_id)
            {
                continue;
            }
            snapshots.push(ViewerSessionSnapshot {
                session_id: session_id.clone(),
                kind: ViewerSessionKind::RemoteRegistry,
                agent_id: session.agent_id.clone(),
                user_id: session.viewer_user_id.clone(),
                user_email: session.viewer_user_email.clone(),
                attached_at: session.attached_at,
                viewer_connected_at: session.viewer_connected_at,
                viewer_last_heartbeat_at: session.viewer_last_heartbeat_at,
            });
        }
    }
    {
        let sessions = state.chat_sessions.read().await;
        for (session_id, session) in sessions.iter() {
            if !is_presence_live(session.viewer_last_heartbeat_at) {
                continue;
            }
            if query
                .agent_id
                .as_deref()
                .is_some_and(|agent_id| agent_id != session.agent_id)
            {
                continue;
            }
            snapshots.push(ViewerSessionSnapshot {
                session_id: session_id.clone(),
                kind: ViewerSessionKind::Chat,
                agent_id: session.agent_id.clone(),
                user_id: session.viewer_user_id.clone(),
                user_email: session.viewer_user_email.clone(),
                attached_at: session.attached_at,
                viewer_connected_at: session.viewer_connected_at,
                viewer_last_heartbeat_at: session.viewer_last_heartbeat_at,
            });
        }
    }

    let mut authorized_snapshots = Vec::new();
    for snapshot in snapshots {
        let device_scope = fetch_device_scope(&state, &snapshot.agent_id).await?;
        if device_scope.organization_id == user_context.organization_id {
            authorized_snapshots.push(snapshot);
        }
    }

    authorized_snapshots
        .sort_by_key(|snapshot| std::cmp::Reverse(snapshot.viewer_last_heartbeat_at));
    Ok(Json(
        authorized_snapshots
            .into_iter()
            .map(viewer_connection_summary_from_snapshot)
            .collect(),
    ))
}

async fn open_desktop_from_shell(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Query(query): Query<SessionCapabilitiesQuery>,
    headers: HeaderMap,
) -> Result<Json<ConnectResponse>, AppError> {
    let shell_session = {
        let sessions = state.active_shell_sessions.read().await;
        sessions.get(&session_id).cloned()
    };
    let Some(shell_session) = shell_session else {
        return Err(AppError::not_found("shell session not found"));
    };
    if shell_session.token != query.token {
        return Err(AppError::unauthorized("invalid shell session token"));
    }

    let api_base = extract_api_base(&headers);
    let response = start_remote_desktop_session(
        &state,
        &shell_session.agent_id,
        api_base,
        "desktop",
        SessionTransportMode::RemoteDesktop,
        shell_session
            .viewer_user_id
            .as_ref()
            .map(|user_id| ViewerIdentity {
                user_id: user_id.clone(),
                user_email: shell_session.viewer_user_email.clone(),
            }),
        None,
        false,
    )
    .await?;
    Ok(Json(response))
}

async fn open_file_transfer_from_shell(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Query(query): Query<SessionCapabilitiesQuery>,
    headers: HeaderMap,
) -> Result<Json<ConnectResponse>, AppError> {
    let shell_session = {
        let sessions = state.active_shell_sessions.read().await;
        sessions.get(&session_id).cloned()
    };
    let Some(shell_session) = shell_session else {
        return Err(AppError::not_found("shell session not found"));
    };
    if shell_session.token != query.token {
        return Err(AppError::unauthorized("invalid shell session token"));
    }

    let api_base = extract_api_base(&headers);
    let response = start_file_transfer_session(
        &state,
        &shell_session.agent_id,
        api_base,
        shell_session
            .viewer_user_id
            .as_ref()
            .map(|user_id| ViewerIdentity {
                user_id: user_id.clone(),
                user_email: shell_session.viewer_user_email.clone(),
            }),
    )
    .await?;
    Ok(Json(response))
}

async fn open_registry_from_shell(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Query(query): Query<SessionCapabilitiesQuery>,
    headers: HeaderMap,
) -> Result<Json<ConnectResponse>, AppError> {
    let shell_session = {
        let sessions = state.active_shell_sessions.read().await;
        sessions.get(&session_id).cloned()
    };
    let Some(shell_session) = shell_session else {
        return Err(AppError::not_found("shell session not found"));
    };
    if shell_session.token != query.token {
        return Err(AppError::unauthorized("invalid shell session token"));
    }

    let api_base = extract_api_base(&headers);
    let response = start_remote_registry_session(
        &state,
        &shell_session.agent_id,
        api_base,
        shell_session
            .viewer_user_id
            .as_ref()
            .map(|user_id| ViewerIdentity {
                user_id: user_id.clone(),
                user_email: shell_session.viewer_user_email.clone(),
            }),
    )
    .await?;
    Ok(Json(response))
}

async fn get_linux_shell_credential_for_shell_session(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Query(query): Query<TokenQuery>,
) -> Result<Json<LinuxShellCredentialResponse>, AppError> {
    let token = query
        .token
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| AppError::unauthorized("missing session token"))?;
    let session = {
        let sessions = state.active_shell_sessions.read().await;
        sessions.get(&session_id).cloned()
    }
    .ok_or_else(|| AppError::not_found("shell session not found"))?;
    if session.token != token {
        return Err(AppError::unauthorized("invalid session token"));
    }
    let Some(user_id) = session.viewer_user_id.as_deref() else {
        return Err(AppError::forbidden("viewer identity unavailable"));
    };

    let body = serde_json::json!({ "userId": user_id });
    let credential: LinuxShellCredentialResponse = api_request(
        &state,
        reqwest::Method::POST,
        &format!(
            "/rmm/devices/{}/linux-shell-credential/reveal-for-user",
            session.agent_id
        ),
        None,
        Some(body),
        true,
        "reveal Linux shell credential for viewer",
    )
    .await?;
    Ok(Json(credential))
}

async fn open_file_transfer_from_session(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Query(query): Query<SessionCapabilitiesQuery>,
    headers: HeaderMap,
) -> Result<Json<ConnectResponse>, AppError> {
    let session = {
        let sessions = state.remote_desktop_sessions.read().await;
        sessions.get(&session_id).cloned()
    };
    let Some(session) = session else {
        return Err(AppError::not_found("session not found"));
    };
    if session.token != query.token {
        return Err(AppError::unauthorized("invalid session token"));
    }

    let api_base = extract_api_base(&headers);
    let response = start_file_transfer_session(
        &state,
        &session.agent_id,
        api_base,
        session
            .viewer_user_id
            .as_ref()
            .map(|user_id| ViewerIdentity {
                user_id: user_id.clone(),
                user_email: session.viewer_user_email.clone(),
            }),
    )
    .await?;
    Ok(Json(response))
}

async fn open_registry_from_session(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Query(query): Query<SessionCapabilitiesQuery>,
    headers: HeaderMap,
) -> Result<Json<ConnectResponse>, AppError> {
    let session = {
        let sessions = state.remote_desktop_sessions.read().await;
        sessions.get(&session_id).cloned()
    };
    let Some(session) = session else {
        return Err(AppError::not_found("session not found"));
    };
    if session.token != query.token {
        return Err(AppError::unauthorized("invalid session token"));
    }

    let api_base = extract_api_base(&headers);
    let response = start_remote_registry_session(
        &state,
        &session.agent_id,
        api_base,
        session
            .viewer_user_id
            .as_ref()
            .map(|user_id| ViewerIdentity {
                user_id: user_id.clone(),
                user_email: session.viewer_user_email.clone(),
            }),
    )
    .await?;
    Ok(Json(response))
}

async fn open_chat_from_session(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Query(query): Query<SessionCapabilitiesQuery>,
    headers: HeaderMap,
) -> Result<Json<ConnectResponse>, AppError> {
    let session = {
        let sessions = state.remote_desktop_sessions.read().await;
        sessions.get(&session_id).cloned()
    };
    let Some(session) = session else {
        return Err(AppError::not_found("session not found"));
    };
    if session.token != query.token {
        return Err(AppError::unauthorized("invalid session token"));
    }

    let api_base = extract_api_base(&headers);
    let response = start_chat_session(
        &state,
        &session.agent_id,
        api_base,
        session
            .viewer_user_id
            .as_ref()
            .map(|user_id| ViewerIdentity {
                user_id: user_id.clone(),
                user_email: session.viewer_user_email.clone(),
            }),
        Some(session_id.clone()),
    )
    .await?;
    Ok(Json(response))
}

async fn get_session_capabilities(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Query(query): Query<SessionCapabilitiesQuery>,
) -> Result<Json<SessionCapabilitiesHttpResponse>, AppError> {
    let session = {
        let sessions = state.remote_desktop_sessions.read().await;
        sessions.get(&session_id).cloned()
    };

    let Some(session) = session else {
        return Err(AppError::not_found("session not found"));
    };

    if session.token != query.token {
        return Err(AppError::unauthorized("invalid session token"));
    }

    Ok(Json(SessionCapabilitiesHttpResponse {
        codecs: session.capabilities.codecs,
        encoding: session.capabilities.encoding,
        transports: session.capabilities.transports,
        display_profiles: session.capabilities.display_profiles,
        selected_display_profile: session.capabilities.selected_display_profile,
        platform: session.capabilities.platform,
        features: session.capabilities.features,
        agent_reflex: session.agent_reflex,
        agent_host: session.agent_host,
        agent_local_addrs: session.agent_local_addrs,
        psk_cert_pem: session.psk_cert_pem,
        relay_url: session.relay_url.clone(),
        e2e_key: session
            .relay_url
            .as_ref()
            .map(|_| BASE64_STANDARD.encode(&session.e2e_key)),
        agent_hostname: session.agent_hostname,
        agent_os: session.agent_os,
        agent_version: session.agent_version,
    }))
}

async fn get_file_transfer_capabilities(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Query(query): Query<SessionCapabilitiesQuery>,
) -> Result<Json<FileTransferSessionCapabilitiesHttpResponse>, AppError> {
    let session = {
        let sessions = state.file_transfer_sessions.read().await;
        sessions.get(&session_id).cloned()
    };

    let Some(session) = session else {
        return Err(AppError::not_found("file transfer session not found"));
    };

    if session.token != query.token {
        return Err(AppError::unauthorized("invalid session token"));
    }

    Ok(Json(FileTransferSessionCapabilitiesHttpResponse {
        transports: session.transports,
        platform: session.platform,
        features: session.features,
        agent_reflex: session.agent_reflex,
        agent_host: session.agent_host,
        agent_local_addrs: session.agent_local_addrs,
        psk_cert_pem: session.psk_cert_pem,
        relay_url: session.relay_url.clone(),
        e2e_key: session
            .relay_url
            .as_ref()
            .map(|_| BASE64_STANDARD.encode(&session.e2e_key)),
        zip_threshold_files: FILE_TRANSFER_DEFAULT_ZIP_THRESHOLD_FILES,
        zip_threshold_bytes: FILE_TRANSFER_DEFAULT_ZIP_THRESHOLD_BYTES,
        max_chunk_bytes: FILE_TRANSFER_DEFAULT_CHUNK_BYTES,
    }))
}

async fn get_registry_capabilities(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Query(query): Query<SessionCapabilitiesQuery>,
) -> Result<Json<RegistrySessionCapabilitiesHttpResponse>, AppError> {
    let session = {
        let sessions = state.remote_registry_sessions.read().await;
        sessions.get(&session_id).cloned()
    };

    let Some(session) = session else {
        return Err(AppError::not_found("remote registry session not found"));
    };

    if session.token != query.token {
        return Err(AppError::unauthorized("invalid session token"));
    }

    Ok(Json(RegistrySessionCapabilitiesHttpResponse {
        transports: session.transports,
        platform: session.platform,
        features: session.features,
        agent_reflex: session.agent_reflex,
        agent_host: session.agent_host,
        agent_local_addrs: session.agent_local_addrs,
        psk_cert_pem: session.psk_cert_pem,
        relay_url: session.relay_url.clone(),
        e2e_key: session
            .relay_url
            .as_ref()
            .map(|_| BASE64_STANDARD.encode(&session.e2e_key)),
    }))
}

async fn get_session_device_info(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Query(query): Query<SessionDeviceInfoQuery>,
) -> Result<Json<SessionDeviceInfoHttpResponse>, AppError> {
    let session = {
        let sessions = state.remote_desktop_sessions.read().await;
        sessions.get(&session_id).cloned()
    };
    let Some(session) = session else {
        return Err(AppError::not_found("session not found"));
    };
    if session.token != query.token {
        return Err(AppError::unauthorized("invalid session token"));
    }

    let should_refresh = parse_refresh_flag(query.refresh.as_deref());
    let mut refreshed = false;
    let mut refresh_error = None;

    let device = if should_refresh {
        match fetch_device_details_for_agent(&state, &session.agent_id).await {
            Ok(device) => {
                refreshed = true;
                device
            }
            Err(err) => {
                refresh_error = Some(err.to_string());
                fetch_device_summary_from_api(&state, &session.agent_id).await?
            }
        }
    } else {
        fetch_device_summary_from_api(&state, &session.agent_id).await?
    };

    Ok(Json(SessionDeviceInfoHttpResponse {
        device,
        refreshed,
        refresh_error,
    }))
}

fn parse_refresh_flag(value: Option<&str>) -> bool {
    let Some(raw) = value else {
        return true;
    };
    let normalized = raw.trim().to_ascii_lowercase();
    !matches!(normalized.as_str(), "0" | "false" | "no" | "off")
}

async fn request_relay(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Query(query): Query<SessionCapabilitiesQuery>,
) -> Result<StatusCode, AppError> {
    let session = {
        let sessions = state.remote_desktop_sessions.read().await;
        sessions.get(&session_id).cloned()
    };

    let Some(session) = session else {
        return Err(AppError::not_found("session not found"));
    };

    if session.token != query.token {
        return Err(AppError::unauthorized("invalid session token"));
    }

    let Some(relay_url) = session.relay_url.clone() else {
        return Err(AppError::bad_request(
            "relay not configured for this session",
        ));
    };
    let e2e_key = BASE64_STANDARD.encode(&session.e2e_key);

    let agent = {
        let agents = state.agents.read().await;
        agents.get(&session.agent_id).cloned()
    };

    let Some(agent) = agent else {
        return Err(AppError::not_found("agent not connected"));
    };

    {
        let mut sessions = state.remote_desktop_sessions.write().await;
        if let Some(existing) = sessions.get_mut(&session_id) {
            existing.attached_at = Some(Instant::now());
        }
    }

    let payload = RelayPreparePayload {
        session_id: session_id.clone(),
        relay_url,
        e2e_key,
        mode: session.mode,
        selected_display_profile: session.capabilities.selected_display_profile.clone(),
        hide_cursor: session.hide_cursor,
    };
    let envelope = OutgoingEnvelope {
        message_type: "relay_prepare",
        data: payload,
    };
    let message = serde_json::to_string(&envelope)
        .context("serialize relay_prepare")
        .map_err(AppError::from)?;
    agent
        .sender
        .send(Message::Text(message))
        .await
        .map_err(|_| AppError::bad_request("agent unavailable"))?;

    Ok(StatusCode::OK)
}

async fn request_file_transfer_relay(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Query(query): Query<SessionCapabilitiesQuery>,
) -> Result<StatusCode, AppError> {
    let session = {
        let sessions = state.file_transfer_sessions.read().await;
        sessions.get(&session_id).cloned()
    };

    let Some(session) = session else {
        return Err(AppError::not_found("file transfer session not found"));
    };

    if session.token != query.token {
        return Err(AppError::unauthorized("invalid session token"));
    }

    let Some(relay_url) = session.relay_url.clone() else {
        return Err(AppError::bad_request(
            "relay not configured for this session",
        ));
    };
    let e2e_key = BASE64_STANDARD.encode(&session.e2e_key);

    let agent = {
        let agents = state.agents.read().await;
        agents.get(&session.agent_id).cloned()
    };

    let Some(agent) = agent else {
        return Err(AppError::not_found("agent not connected"));
    };

    {
        let mut sessions = state.file_transfer_sessions.write().await;
        if let Some(existing) = sessions.get_mut(&session_id) {
            existing.attached_at = Some(Instant::now());
        }
    }

    let payload = RelayPreparePayload {
        session_id: session_id.clone(),
        relay_url,
        e2e_key,
        mode: SessionTransportMode::FileTransfer,
        selected_display_profile: None,
        hide_cursor: false,
    };
    let envelope = OutgoingEnvelope {
        message_type: "relay_prepare",
        data: payload,
    };
    let message = serde_json::to_string(&envelope)
        .context("serialize file transfer relay_prepare")
        .map_err(AppError::from)?;
    agent
        .sender
        .send(Message::Text(message))
        .await
        .map_err(|_| AppError::bad_request("agent unavailable"))?;

    Ok(StatusCode::OK)
}

async fn request_registry_relay(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Query(query): Query<SessionCapabilitiesQuery>,
) -> Result<StatusCode, AppError> {
    let session = {
        let sessions = state.remote_registry_sessions.read().await;
        sessions.get(&session_id).cloned()
    };

    let Some(session) = session else {
        return Err(AppError::not_found("remote registry session not found"));
    };

    if session.token != query.token {
        return Err(AppError::unauthorized("invalid session token"));
    }

    let Some(relay_url) = session.relay_url.clone() else {
        return Err(AppError::bad_request(
            "relay not configured for this session",
        ));
    };
    let e2e_key = BASE64_STANDARD.encode(&session.e2e_key);

    let agent = {
        let agents = state.agents.read().await;
        agents.get(&session.agent_id).cloned()
    };

    let Some(agent) = agent else {
        return Err(AppError::not_found("agent not connected"));
    };

    {
        let mut sessions = state.remote_registry_sessions.write().await;
        if let Some(existing) = sessions.get_mut(&session_id) {
            existing.attached_at = Some(Instant::now());
        }
    }

    let payload = RelayPreparePayload {
        session_id: session_id.clone(),
        relay_url,
        e2e_key,
        mode: SessionTransportMode::RemoteRegistry,
        selected_display_profile: None,
        hide_cursor: false,
    };
    let envelope = OutgoingEnvelope {
        message_type: "relay_prepare",
        data: payload,
    };
    let message = serde_json::to_string(&envelope)
        .context("serialize registry relay_prepare")
        .map_err(AppError::from)?;
    agent
        .sender
        .send(Message::Text(message))
        .await
        .map_err(|_| AppError::bad_request("agent unavailable"))?;

    Ok(StatusCode::OK)
}

async fn viewer_reflex(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Query(query): Query<ViewerReflexQuery>,
    Json(payload): Json<ViewerReflexRequest>,
) -> Result<StatusCode, AppError> {
    let session = {
        let sessions = state.remote_desktop_sessions.read().await;
        sessions.get(&session_id).cloned()
    };

    let Some(session) = session else {
        return Err(AppError::not_found("session not found"));
    };

    if session.token != query.token {
        return Err(AppError::unauthorized("invalid session token"));
    }

    let viewer_reflex = ReflexAddress {
        ip: payload.ip,
        port: payload.port,
    };

    {
        let mut sessions = state.remote_desktop_sessions.write().await;
        if let Some(existing) = sessions.get_mut(&session_id) {
            existing.viewer_reflex = Some(viewer_reflex.clone());
            existing.attached_at = Some(Instant::now());
        }
    }

    // If agent is connected, notify it to punch toward viewer
    let agent = {
        let agents = state.agents.read().await;
        agents.get(&session.agent_id).cloned()
    };
    if let Some(agent) = agent {
        let payload = PunchStartPayload {
            session_id: session_id.clone(),
            peer_reflex: viewer_reflex,
        };
        let envelope = OutgoingEnvelope {
            message_type: "punch_start",
            data: payload,
        };
        let message = serde_json::to_string(&envelope).context("serialize punch_start")?;
        let _ = agent.sender.send(Message::Text(message)).await;
    }

    Ok(StatusCode::OK)
}

async fn viewer_connected(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Query(query): Query<SessionCapabilitiesQuery>,
) -> Result<StatusCode, AppError> {
    let session = {
        let sessions = state.remote_desktop_sessions.read().await;
        sessions.get(&session_id).cloned()
    };
    let Some(session) = session else {
        return Err(AppError::not_found("session not found"));
    };
    if session.token != query.token {
        return Err(AppError::unauthorized("invalid session token"));
    }
    let now = Instant::now();
    let mut sessions = state.remote_desktop_sessions.write().await;
    if let Some(existing) = sessions.get_mut(&session_id) {
        existing.attached_at = Some(now);
        if existing.viewer_connected_at.is_none() {
            existing.viewer_connected_at = Some(now);
        }
        existing.viewer_last_heartbeat_at = Some(now);
    }
    Ok(StatusCode::OK)
}

async fn viewer_heartbeat(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Query(query): Query<SessionCapabilitiesQuery>,
) -> Result<StatusCode, AppError> {
    let session = {
        let sessions = state.remote_desktop_sessions.read().await;
        sessions.get(&session_id).cloned()
    };
    let Some(session) = session else {
        return Err(AppError::not_found("session not found"));
    };
    if session.token != query.token {
        return Err(AppError::unauthorized("invalid session token"));
    }
    let now = Instant::now();
    let mut sessions = state.remote_desktop_sessions.write().await;
    if let Some(existing) = sessions.get_mut(&session_id) {
        existing.attached_at = Some(now);
        existing.viewer_last_heartbeat_at = Some(now);
    }
    Ok(StatusCode::OK)
}

async fn file_transfer_viewer_reflex(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Query(query): Query<ViewerReflexQuery>,
    Json(payload): Json<ViewerReflexRequest>,
) -> Result<StatusCode, AppError> {
    let session = {
        let sessions = state.file_transfer_sessions.read().await;
        sessions.get(&session_id).cloned()
    };

    let Some(session) = session else {
        return Err(AppError::not_found("file transfer session not found"));
    };

    if session.token != query.token {
        return Err(AppError::unauthorized("invalid session token"));
    }

    let viewer_reflex = ReflexAddress {
        ip: payload.ip,
        port: payload.port,
    };

    {
        let mut sessions = state.file_transfer_sessions.write().await;
        if let Some(existing) = sessions.get_mut(&session_id) {
            existing.viewer_reflex = Some(viewer_reflex.clone());
            existing.attached_at = Some(Instant::now());
        }
    }

    let agent = {
        let agents = state.agents.read().await;
        agents.get(&session.agent_id).cloned()
    };
    if let Some(agent) = agent {
        let payload = PunchStartPayload {
            session_id: session_id.clone(),
            peer_reflex: viewer_reflex,
        };
        let envelope = OutgoingEnvelope {
            message_type: "punch_start",
            data: payload,
        };
        let message = serde_json::to_string(&envelope).context("serialize punch_start")?;
        let _ = agent.sender.send(Message::Text(message)).await;
    }

    Ok(StatusCode::OK)
}

async fn file_transfer_viewer_connected(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Query(query): Query<SessionCapabilitiesQuery>,
) -> Result<StatusCode, AppError> {
    let session = {
        let sessions = state.file_transfer_sessions.read().await;
        sessions.get(&session_id).cloned()
    };
    let Some(session) = session else {
        return Err(AppError::not_found("file transfer session not found"));
    };
    if session.token != query.token {
        return Err(AppError::unauthorized("invalid session token"));
    }
    let now = Instant::now();
    let mut sessions = state.file_transfer_sessions.write().await;
    if let Some(existing) = sessions.get_mut(&session_id) {
        existing.attached_at = Some(now);
        if existing.viewer_connected_at.is_none() {
            existing.viewer_connected_at = Some(now);
        }
        existing.viewer_last_heartbeat_at = Some(now);
    }
    Ok(StatusCode::OK)
}

async fn file_transfer_viewer_heartbeat(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Query(query): Query<SessionCapabilitiesQuery>,
) -> Result<StatusCode, AppError> {
    let session = {
        let sessions = state.file_transfer_sessions.read().await;
        sessions.get(&session_id).cloned()
    };
    let Some(session) = session else {
        return Err(AppError::not_found("file transfer session not found"));
    };
    if session.token != query.token {
        return Err(AppError::unauthorized("invalid session token"));
    }
    let now = Instant::now();
    let mut sessions = state.file_transfer_sessions.write().await;
    if let Some(existing) = sessions.get_mut(&session_id) {
        existing.attached_at = Some(now);
        existing.viewer_last_heartbeat_at = Some(now);
    }
    Ok(StatusCode::OK)
}

async fn get_chat_capabilities(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Query(query): Query<SessionCapabilitiesQuery>,
) -> Result<Json<ChatSessionCapabilitiesHttpResponse>, AppError> {
    let sessions = state.chat_sessions.read().await;
    let Some(session) = sessions.get(&session_id) else {
        return Err(AppError::not_found("chat session not found"));
    };
    if session.token != query.token {
        return Err(AppError::unauthorized("invalid session token"));
    }
    Ok(Json(ChatSessionCapabilitiesHttpResponse {
        transports: session.transports.clone(),
        platform: session.platform,
        features: session.features.clone(),
        agent_reflex: session.agent_reflex.clone(),
        agent_host: session.agent_host.clone(),
        agent_local_addrs: session.agent_local_addrs.clone(),
        psk_cert_pem: session.psk_cert_pem.clone(),
        relay_url: session.relay_url.clone(),
        e2e_key: session
            .relay_url
            .as_ref()
            .map(|_| BASE64_STANDARD.encode(&session.e2e_key)),
    }))
}

async fn request_chat_relay(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Query(query): Query<SessionCapabilitiesQuery>,
) -> Result<StatusCode, AppError> {
    let agent_id = {
        let sessions = state.chat_sessions.read().await;
        let Some(existing) = sessions.get(&session_id) else {
            return Err(AppError::not_found("chat session not found"));
        };
        if existing.token != query.token {
            return Err(AppError::unauthorized("invalid session token"));
        }
        let relay_url = state.config.relay_url.clone().ok_or_else(|| {
            AppError::service_unavailable("relay unavailable on server configuration")
        })?;
        if existing.relay_url.as_ref() != Some(&relay_url) {
            return Err(AppError::bad_request("relay url mismatch"));
        }
        existing.agent_id.clone()
    };

    let agent = {
        let agents = state.agents.read().await;
        agents.get(&agent_id).cloned()
    };
    let Some(agent) = agent else {
        return Err(AppError::bad_request("agent unavailable"));
    };

    let (relay_url, e2e_key) = {
        let sessions = state.chat_sessions.read().await;
        let Some(existing) = sessions.get(&session_id) else {
            return Err(AppError::not_found("chat session not found"));
        };
        (
            existing
                .relay_url
                .clone()
                .ok_or_else(|| AppError::bad_request("relay url missing"))?,
            BASE64_STANDARD.encode(&existing.e2e_key),
        )
    };

    let payload = RelayPreparePayload {
        session_id: session_id.clone(),
        relay_url,
        e2e_key,
        mode: SessionTransportMode::Chat,
        selected_display_profile: None,
        hide_cursor: false,
    };
    let envelope = OutgoingEnvelope {
        message_type: "relay_prepare",
        data: payload,
    };
    let message = serde_json::to_string(&envelope).context("serialize chat relay_prepare")?;
    agent
        .sender
        .send(Message::Text(message))
        .await
        .map_err(|_| AppError::bad_request("agent unavailable"))?;

    let mut sessions = state.chat_sessions.write().await;
    if let Some(existing) = sessions.get_mut(&session_id) {
        existing.attached_at = Some(Instant::now());
        existing.viewer_last_heartbeat_at = Some(Instant::now());
    }
    Ok(StatusCode::OK)
}

async fn chat_viewer_reflex(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Query(query): Query<ViewerReflexQuery>,
    Json(payload): Json<ViewerReflexRequest>,
) -> Result<StatusCode, AppError> {
    let session = {
        let sessions = state.chat_sessions.read().await;
        sessions.get(&session_id).cloned()
    };

    let Some(session) = session else {
        return Err(AppError::not_found("chat session not found"));
    };

    if session.token != query.token {
        return Err(AppError::unauthorized("invalid session token"));
    }

    let viewer_reflex = ReflexAddress {
        ip: payload.ip,
        port: payload.port,
    };

    {
        let mut sessions = state.chat_sessions.write().await;
        if let Some(existing) = sessions.get_mut(&session_id) {
            existing.viewer_reflex = Some(viewer_reflex.clone());
            existing.attached_at = Some(Instant::now());
        }
    }

    let agent = {
        let agents = state.agents.read().await;
        agents.get(&session.agent_id).cloned()
    };
    if let Some(agent) = agent {
        let payload = PunchStartPayload {
            session_id: session_id.clone(),
            peer_reflex: viewer_reflex,
        };
        let envelope = OutgoingEnvelope {
            message_type: "punch_start",
            data: payload,
        };
        let message = serde_json::to_string(&envelope).context("serialize punch_start")?;
        let _ = agent.sender.send(Message::Text(message)).await;
    }

    Ok(StatusCode::OK)
}

async fn chat_viewer_connected(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Query(query): Query<SessionCapabilitiesQuery>,
) -> Result<StatusCode, AppError> {
    let session = {
        let sessions = state.chat_sessions.read().await;
        sessions.get(&session_id).cloned()
    };
    let Some(session) = session else {
        return Err(AppError::not_found("chat session not found"));
    };
    if session.token != query.token {
        return Err(AppError::unauthorized("invalid session token"));
    }
    let now = Instant::now();
    let mut sessions = state.chat_sessions.write().await;
    if let Some(existing) = sessions.get_mut(&session_id) {
        existing.attached_at = Some(now);
        if existing.viewer_connected_at.is_none() {
            existing.viewer_connected_at = Some(now);
        }
        existing.viewer_last_heartbeat_at = Some(now);
    }
    Ok(StatusCode::OK)
}

async fn chat_viewer_heartbeat(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Query(query): Query<SessionCapabilitiesQuery>,
) -> Result<StatusCode, AppError> {
    let session = {
        let sessions = state.chat_sessions.read().await;
        sessions.get(&session_id).cloned()
    };
    let Some(session) = session else {
        return Err(AppError::not_found("chat session not found"));
    };
    if session.token != query.token {
        return Err(AppError::unauthorized("invalid session token"));
    }
    let now = Instant::now();
    let mut sessions = state.chat_sessions.write().await;
    if let Some(existing) = sessions.get_mut(&session_id) {
        existing.attached_at = Some(now);
        existing.viewer_last_heartbeat_at = Some(now);
    }
    Ok(StatusCode::OK)
}

async fn cleanup_ai_runner_session_internal(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<AiRunnerSessionCleanupRequest>,
) -> Result<Json<Value>, AppError> {
    require_internal_server_key(&state.config, &headers)?;
    let session_id = payload.session_id.trim().to_string();
    let agent_id = payload.agent_id.trim().to_string();
    let kind = payload.kind.trim().to_ascii_lowercase();
    if session_id.is_empty() || agent_id.is_empty() {
        return Err(AppError::bad_request("sessionId and agentId are required"));
    }

    let removed = match kind.as_str() {
        "desktop" | "remote_desktop" => {
            let removed = {
                let mut sessions = state.remote_desktop_sessions.write().await;
                let existing_agent_id = sessions
                    .get(&session_id)
                    .map(|session| session.agent_id.clone());
                if let Some(existing_agent_id) = existing_agent_id.as_deref() {
                    if existing_agent_id != agent_id {
                        return Err(AppError::forbidden("session belongs to another agent"));
                    }
                }
                existing_agent_id.is_some() && sessions.remove(&session_id).is_some()
            };
            let child_chat_ids = {
                let sessions = state.chat_sessions.read().await;
                sessions
                    .iter()
                    .filter_map(|(chat_session_id, chat)| {
                        if chat.parent_desktop_session_id.as_deref() == Some(session_id.as_str())
                            && chat.agent_id == agent_id
                        {
                            Some(chat_session_id.clone())
                        } else {
                            None
                        }
                    })
                    .collect::<Vec<_>>()
            };
            if !child_chat_ids.is_empty() {
                let mut sessions = state.chat_sessions.write().await;
                for chat_session_id in &child_chat_ids {
                    sessions.remove(chat_session_id);
                }
            }
            if removed {
                notify_agent_session_end(&state, &agent_id, &session_id, "remote_desktop").await;
            }
            for chat_session_id in child_chat_ids {
                notify_agent_session_end(&state, &agent_id, &chat_session_id, "chat").await;
            }
            removed
        }
        "shell" => {
            let removed = {
                let mut sessions = state.active_shell_sessions.write().await;
                let existing_agent_id = sessions
                    .get(&session_id)
                    .map(|session| session.agent_id.clone());
                if let Some(existing_agent_id) = existing_agent_id.as_deref() {
                    if existing_agent_id != agent_id {
                        return Err(AppError::forbidden("session belongs to another agent"));
                    }
                }
                existing_agent_id.is_some() && sessions.remove(&session_id).is_some()
            };
            if removed {
                notify_agent_session_end(&state, &agent_id, &session_id, "shell").await;
            }
            removed
        }
        "chat" => {
            let removed = {
                let mut sessions = state.chat_sessions.write().await;
                let existing_agent_id = sessions
                    .get(&session_id)
                    .map(|session| session.agent_id.clone());
                if let Some(existing_agent_id) = existing_agent_id.as_deref() {
                    if existing_agent_id != agent_id {
                        return Err(AppError::forbidden("session belongs to another agent"));
                    }
                }
                existing_agent_id.is_some() && sessions.remove(&session_id).is_some()
            };
            if removed {
                notify_agent_session_end(&state, &agent_id, &session_id, "chat").await;
            }
            removed
        }
        _ => return Err(AppError::bad_request("unsupported session kind")),
    };

    info!(
        session_id = %session_id,
        agent_id = %agent_id,
        kind = %kind,
        removed,
        "AI runner stale session cleanup requested"
    );
    Ok(Json(serde_json::json!({ "ok": true, "removed": removed })))
}

async fn end_chat_session(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Query(query): Query<SessionCapabilitiesQuery>,
) -> Result<StatusCode, AppError> {
    let (agent_id, token_ok, viewer_identity, parent_desktop_session_id) = {
        let sessions = state.chat_sessions.read().await;
        let Some(existing) = sessions.get(&session_id) else {
            return Err(AppError::not_found("chat session not found"));
        };
        (
            existing.agent_id.clone(),
            existing.token == query.token,
            existing
                .viewer_user_id
                .as_ref()
                .map(|user_id| ViewerIdentity {
                    user_id: user_id.clone(),
                    user_email: existing.viewer_user_email.clone(),
                }),
            existing.parent_desktop_session_id.clone(),
        )
    };
    if !token_ok {
        return Err(AppError::unauthorized("invalid session token"));
    }
    {
        let mut sessions = state.chat_sessions.write().await;
        let Some(_) = sessions.get(&session_id) else {
            return Err(AppError::not_found("chat session not found"));
        };
        sessions.remove(&session_id);
    }
    info!(session_id = %session_id, agent_id = %agent_id, "chat session ended by viewer");

    notify_agent_session_end(&state, &agent_id, &session_id, "chat").await;
    let device_scope = fetch_device_scope(&state, &agent_id).await?;
    log_audit_event(
        &state,
        Some(&device_scope.organization_id),
        device_scope.customer_id.as_deref(),
        device_scope.site_id.as_deref(),
        Some(&agent_id),
        viewer_identity.as_ref(),
        "chat.end",
        "rmm_device",
        Some(&agent_id),
        None,
        "success",
        Some(&session_id),
        serde_json::json!({
            "endedBy": "viewer",
            "parentDesktopSessionId": parent_desktop_session_id,
        }),
    )
    .await?;
    Ok(StatusCode::OK)
}

async fn registry_viewer_reflex(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Query(query): Query<ViewerReflexQuery>,
    Json(payload): Json<ViewerReflexRequest>,
) -> Result<StatusCode, AppError> {
    let session = {
        let sessions = state.remote_registry_sessions.read().await;
        sessions.get(&session_id).cloned()
    };

    let Some(session) = session else {
        return Err(AppError::not_found("remote registry session not found"));
    };

    if session.token != query.token {
        return Err(AppError::unauthorized("invalid session token"));
    }

    let viewer_reflex = ReflexAddress {
        ip: payload.ip,
        port: payload.port,
    };

    {
        let mut sessions = state.remote_registry_sessions.write().await;
        if let Some(existing) = sessions.get_mut(&session_id) {
            existing.viewer_reflex = Some(viewer_reflex.clone());
            existing.attached_at = Some(Instant::now());
        }
    }

    let agent = {
        let agents = state.agents.read().await;
        agents.get(&session.agent_id).cloned()
    };
    if let Some(agent) = agent {
        let payload = PunchStartPayload {
            session_id: session_id.clone(),
            peer_reflex: viewer_reflex,
        };
        let envelope = OutgoingEnvelope {
            message_type: "punch_start",
            data: payload,
        };
        let message = serde_json::to_string(&envelope).context("serialize punch_start")?;
        let _ = agent.sender.send(Message::Text(message)).await;
    }

    Ok(StatusCode::OK)
}

async fn registry_viewer_connected(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Query(query): Query<SessionCapabilitiesQuery>,
) -> Result<StatusCode, AppError> {
    let session = {
        let sessions = state.remote_registry_sessions.read().await;
        sessions.get(&session_id).cloned()
    };
    let Some(session) = session else {
        return Err(AppError::not_found("remote registry session not found"));
    };
    if session.token != query.token {
        return Err(AppError::unauthorized("invalid session token"));
    }
    let now = Instant::now();
    let mut sessions = state.remote_registry_sessions.write().await;
    if let Some(existing) = sessions.get_mut(&session_id) {
        existing.attached_at = Some(now);
        if existing.viewer_connected_at.is_none() {
            existing.viewer_connected_at = Some(now);
        }
        existing.viewer_last_heartbeat_at = Some(now);
    }
    Ok(StatusCode::OK)
}

async fn registry_viewer_heartbeat(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Query(query): Query<SessionCapabilitiesQuery>,
) -> Result<StatusCode, AppError> {
    let session = {
        let sessions = state.remote_registry_sessions.read().await;
        sessions.get(&session_id).cloned()
    };
    let Some(session) = session else {
        return Err(AppError::not_found("remote registry session not found"));
    };
    if session.token != query.token {
        return Err(AppError::unauthorized("invalid session token"));
    }
    let now = Instant::now();
    let mut sessions = state.remote_registry_sessions.write().await;
    if let Some(existing) = sessions.get_mut(&session_id) {
        existing.attached_at = Some(now);
        existing.viewer_last_heartbeat_at = Some(now);
    }
    Ok(StatusCode::OK)
}

async fn end_remote_desktop_session(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Query(query): Query<SessionCapabilitiesQuery>,
) -> Result<StatusCode, AppError> {
    let (agent_id, token_ok, viewer_identity) = {
        let sessions = state.remote_desktop_sessions.read().await;
        let Some(existing) = sessions.get(&session_id) else {
            return Err(AppError::not_found("session not found"));
        };
        (
            existing.agent_id.clone(),
            existing.token == query.token,
            existing
                .viewer_user_id
                .as_ref()
                .map(|user_id| ViewerIdentity {
                    user_id: user_id.clone(),
                    user_email: existing.viewer_user_email.clone(),
                }),
        )
    };
    if !token_ok {
        return Err(AppError::unauthorized("invalid session token"));
    }
    {
        let mut sessions = state.remote_desktop_sessions.write().await;
        let Some(_) = sessions.get(&session_id) else {
            return Err(AppError::not_found("session not found"));
        };
        sessions.remove(&session_id);
    }
    let child_chat_ids = {
        let sessions = state.chat_sessions.read().await;
        sessions
            .iter()
            .filter_map(|(chat_session_id, chat)| {
                if chat.parent_desktop_session_id.as_deref() == Some(session_id.as_str()) {
                    Some(chat_session_id.clone())
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
    };
    if !child_chat_ids.is_empty() {
        let mut sessions = state.chat_sessions.write().await;
        for chat_session_id in &child_chat_ids {
            sessions.remove(chat_session_id);
        }
    }
    info!(session_id = %session_id, agent_id = %agent_id, "remote desktop session ended by viewer");

    // Proactively notify agent to teardown pipelines even if the transport stream doesn't
    // observe a socket close promptly (prevents lingering DXGI capture + stuck relay stream).
    let agent = {
        let agents = state.agents.read().await;
        agents.get(&agent_id).cloned()
    };
    if let Some(agent) = agent {
        let payload = serde_json::json!({
            "session_id": session_id.clone(),
            "kind": "remote_desktop",
        });
        let envelope = OutgoingEnvelope {
            message_type: "session_end",
            data: payload,
        };
        if let Ok(message) = serde_json::to_string(&envelope) {
            let _ = agent.sender.send(Message::Text(message)).await;
        }
    }
    for chat_session_id in child_chat_ids {
        notify_agent_session_end(&state, &agent_id, &chat_session_id, "chat").await;
    }
    let device_scope = fetch_device_scope(&state, &agent_id).await?;
    log_audit_event(
        &state,
        Some(&device_scope.organization_id),
        device_scope.customer_id.as_deref(),
        device_scope.site_id.as_deref(),
        Some(&agent_id),
        viewer_identity.as_ref(),
        "remote_desktop.end",
        "rmm_device",
        Some(&agent_id),
        None,
        "success",
        Some(&session_id),
        serde_json::json!({
            "endedBy": "viewer",
        }),
    )
    .await?;
    Ok(StatusCode::OK)
}

async fn end_file_transfer_session(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Query(query): Query<SessionCapabilitiesQuery>,
) -> Result<StatusCode, AppError> {
    let (agent_id, token_ok, viewer_identity) = {
        let sessions = state.file_transfer_sessions.read().await;
        let Some(existing) = sessions.get(&session_id) else {
            return Err(AppError::not_found("file transfer session not found"));
        };
        (
            existing.agent_id.clone(),
            existing.token == query.token,
            existing
                .viewer_user_id
                .as_ref()
                .map(|user_id| ViewerIdentity {
                    user_id: user_id.clone(),
                    user_email: existing.viewer_user_email.clone(),
                }),
        )
    };
    if !token_ok {
        return Err(AppError::unauthorized("invalid session token"));
    }
    {
        let mut sessions = state.file_transfer_sessions.write().await;
        let Some(_) = sessions.get(&session_id) else {
            return Err(AppError::not_found("file transfer session not found"));
        };
        sessions.remove(&session_id);
    }
    info!(session_id = %session_id, agent_id = %agent_id, "file transfer session ended by viewer");

    let agent = {
        let agents = state.agents.read().await;
        agents.get(&agent_id).cloned()
    };
    if let Some(agent) = agent {
        let payload = serde_json::json!({
            "session_id": session_id.clone(),
            "kind": "file_transfer",
        });
        let envelope = OutgoingEnvelope {
            message_type: "session_end",
            data: payload,
        };
        if let Ok(message) = serde_json::to_string(&envelope) {
            let _ = agent.sender.send(Message::Text(message)).await;
        }
    }
    let device_scope = fetch_device_scope(&state, &agent_id).await?;
    log_audit_event(
        &state,
        Some(&device_scope.organization_id),
        device_scope.customer_id.as_deref(),
        device_scope.site_id.as_deref(),
        Some(&agent_id),
        viewer_identity.as_ref(),
        "file_transfer.end",
        "rmm_device",
        Some(&agent_id),
        None,
        "success",
        Some(&session_id),
        serde_json::json!({
            "endedBy": "viewer",
        }),
    )
    .await?;
    Ok(StatusCode::OK)
}

async fn end_remote_registry_session(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Query(query): Query<SessionCapabilitiesQuery>,
) -> Result<StatusCode, AppError> {
    let (agent_id, token_ok, viewer_identity) = {
        let sessions = state.remote_registry_sessions.read().await;
        let Some(existing) = sessions.get(&session_id) else {
            return Err(AppError::not_found("remote registry session not found"));
        };
        (
            existing.agent_id.clone(),
            existing.token == query.token,
            existing
                .viewer_user_id
                .as_ref()
                .map(|user_id| ViewerIdentity {
                    user_id: user_id.clone(),
                    user_email: existing.viewer_user_email.clone(),
                }),
        )
    };
    if !token_ok {
        return Err(AppError::unauthorized("invalid session token"));
    }
    {
        let mut sessions = state.remote_registry_sessions.write().await;
        let Some(_) = sessions.get(&session_id) else {
            return Err(AppError::not_found("remote registry session not found"));
        };
        sessions.remove(&session_id);
    }
    info!(session_id = %session_id, agent_id = %agent_id, "remote registry session ended by viewer");

    let agent = {
        let agents = state.agents.read().await;
        agents.get(&agent_id).cloned()
    };
    if let Some(agent) = agent {
        let payload = serde_json::json!({
            "session_id": session_id.clone(),
            "kind": "remote_registry",
        });
        let envelope = OutgoingEnvelope {
            message_type: "session_end",
            data: payload,
        };
        if let Ok(message) = serde_json::to_string(&envelope) {
            let _ = agent.sender.send(Message::Text(message)).await;
        }
    }
    let device_scope = fetch_device_scope(&state, &agent_id).await?;
    log_audit_event(
        &state,
        Some(&device_scope.organization_id),
        device_scope.customer_id.as_deref(),
        device_scope.site_id.as_deref(),
        Some(&agent_id),
        viewer_identity.as_ref(),
        "registry.end",
        "rmm_device",
        Some(&agent_id),
        None,
        "success",
        Some(&session_id),
        serde_json::json!({
            "endedBy": "viewer",
        }),
    )
    .await?;
    Ok(StatusCode::OK)
}

async fn end_shell_session(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Query(query): Query<SessionCapabilitiesQuery>,
) -> Result<StatusCode, AppError> {
    let (agent_id, token_ok, viewer_identity) = {
        let sessions = state.active_shell_sessions.read().await;
        let Some(existing) = sessions.get(&session_id) else {
            return Err(AppError::not_found("shell session not found"));
        };
        (
            existing.agent_id.clone(),
            existing.token == query.token,
            existing
                .viewer_user_id
                .as_ref()
                .map(|user_id| ViewerIdentity {
                    user_id: user_id.clone(),
                    user_email: existing.viewer_user_email.clone(),
                }),
        )
    };
    if !token_ok {
        return Err(AppError::unauthorized("invalid shell session token"));
    }
    {
        let mut sessions = state.active_shell_sessions.write().await;
        let Some(_) = sessions.get(&session_id) else {
            return Err(AppError::not_found("shell session not found"));
        };
        sessions.remove(&session_id);
    }
    info!(session_id = %session_id, agent_id = %agent_id, "shell session ended by viewer");

    let agent = {
        let agents = state.agents.read().await;
        agents.get(&agent_id).cloned()
    };
    if let Some(agent) = agent {
        let payload = serde_json::json!({
            "session_id": session_id.clone(),
            "kind": "shell",
        });
        let envelope = OutgoingEnvelope {
            message_type: "session_end",
            data: payload,
        };
        if let Ok(message) = serde_json::to_string(&envelope) {
            let _ = agent.sender.send(Message::Text(message)).await;
        }
    }
    let device_scope = fetch_device_scope(&state, &agent_id).await?;
    log_audit_event(
        &state,
        Some(&device_scope.organization_id),
        device_scope.customer_id.as_deref(),
        device_scope.site_id.as_deref(),
        Some(&agent_id),
        viewer_identity.as_ref(),
        "shell.end",
        "rmm_device",
        Some(&agent_id),
        None,
        "success",
        Some(&session_id),
        serde_json::json!({
            "endedBy": "viewer",
        }),
    )
    .await?;
    Ok(StatusCode::OK)
}

async fn get_shell_capabilities(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Query(query): Query<SessionCapabilitiesQuery>,
) -> Result<Json<ShellSessionCapabilitiesHttpResponse>, AppError> {
    let session = {
        let sessions = state.active_shell_sessions.read().await;
        sessions.get(&session_id).cloned()
    };
    let Some(session) = session else {
        return Err(AppError::not_found("shell session not found"));
    };
    if session.token != query.token {
        return Err(AppError::unauthorized("invalid session token"));
    }
    Ok(Json(ShellSessionCapabilitiesHttpResponse {
        transports: session.transports,
        platform: session.platform,
        features: session.features,
        agent_reflex: session.agent_reflex,
        agent_host: session.agent_host,
        agent_local_addrs: session.agent_local_addrs,
        psk_cert_pem: session.psk_cert_pem,
        relay_url: session.relay_url.clone(),
        e2e_key: session
            .relay_url
            .as_ref()
            .map(|_| BASE64_STANDARD.encode(&session.e2e_key)),
    }))
}

async fn shell_viewer_reflex(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Query(query): Query<ViewerReflexQuery>,
    Json(payload): Json<ViewerReflexRequest>,
) -> Result<StatusCode, AppError> {
    let session = {
        let sessions = state.active_shell_sessions.read().await;
        sessions.get(&session_id).cloned()
    };
    let Some(session) = session else {
        return Err(AppError::not_found("shell session not found"));
    };
    if session.token != query.token {
        return Err(AppError::unauthorized("invalid session token"));
    }

    let viewer_reflex = ReflexAddress {
        ip: payload.ip,
        port: payload.port,
    };

    {
        let mut sessions = state.active_shell_sessions.write().await;
        if let Some(existing) = sessions.get_mut(&session_id) {
            existing.viewer_reflex = Some(viewer_reflex.clone());
            existing.attached_at = Some(Instant::now());
        }
    }

    let agent = {
        let agents = state.agents.read().await;
        agents.get(&session.agent_id).cloned()
    };
    if let Some(agent) = agent {
        let payload = PunchStartPayload {
            session_id: session_id.clone(),
            peer_reflex: viewer_reflex,
        };
        let envelope = OutgoingEnvelope {
            message_type: "punch_start",
            data: payload,
        };
        if let Ok(message) = serde_json::to_string(&envelope) {
            let _ = agent.sender.send(Message::Text(message)).await;
        }
    }
    Ok(StatusCode::OK)
}

async fn shell_viewer_connected(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Query(query): Query<SessionCapabilitiesQuery>,
) -> Result<StatusCode, AppError> {
    let session = {
        let sessions = state.active_shell_sessions.read().await;
        sessions.get(&session_id).cloned()
    };
    let Some(session) = session else {
        return Err(AppError::not_found("shell session not found"));
    };
    if session.token != query.token {
        return Err(AppError::unauthorized("invalid shell session token"));
    }
    let now = Instant::now();
    let mut sessions = state.active_shell_sessions.write().await;
    if let Some(existing) = sessions.get_mut(&session_id) {
        existing.attached_at = Some(now);
        if existing.viewer_connected_at.is_none() {
            existing.viewer_connected_at = Some(now);
        }
        existing.viewer_last_heartbeat_at = Some(now);
    }
    Ok(StatusCode::OK)
}

async fn shell_viewer_heartbeat(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Query(query): Query<SessionCapabilitiesQuery>,
) -> Result<StatusCode, AppError> {
    let session = {
        let sessions = state.active_shell_sessions.read().await;
        sessions.get(&session_id).cloned()
    };
    let Some(session) = session else {
        return Err(AppError::not_found("shell session not found"));
    };
    if session.token != query.token {
        return Err(AppError::unauthorized("invalid shell session token"));
    }
    let now = Instant::now();
    let mut sessions = state.active_shell_sessions.write().await;
    if let Some(existing) = sessions.get_mut(&session_id) {
        existing.attached_at = Some(now);
        existing.viewer_last_heartbeat_at = Some(now);
    }
    Ok(StatusCode::OK)
}

async fn request_shell_relay(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Query(query): Query<SessionCapabilitiesQuery>,
) -> Result<StatusCode, AppError> {
    let session = {
        let sessions = state.active_shell_sessions.read().await;
        sessions.get(&session_id).cloned()
    };
    let Some(session) = session else {
        return Err(AppError::not_found("shell session not found"));
    };
    if session.token != query.token {
        return Err(AppError::unauthorized("invalid shell session token"));
    };
    if session.relay_url.is_none() {
        return Err(AppError::bad_request("shell relay not configured"));
    }

    let relay_url = session
        .relay_url
        .clone()
        .ok_or_else(|| AppError::bad_request("shell relay not configured"))?;
    let e2e_key = BASE64_STANDARD.encode(&session.e2e_key);

    let agent = {
        let agents = state.agents.read().await;
        agents.get(&session.agent_id).cloned()
    };
    let Some(agent) = agent else {
        return Err(AppError::not_found("agent not connected"));
    };

    {
        let mut sessions = state.active_shell_sessions.write().await;
        if let Some(existing) = sessions.get_mut(&session_id) {
            existing.attached_at = Some(Instant::now());
        }
    }

    let payload = RelayPreparePayload {
        session_id: session_id.clone(),
        relay_url,
        e2e_key,
        mode: SessionTransportMode::Shell,
        selected_display_profile: None,
        hide_cursor: false,
    };
    let envelope = OutgoingEnvelope {
        message_type: "relay_prepare",
        data: payload,
    };
    let message = serde_json::to_string(&envelope)
        .context("serialize shell relay_prepare")
        .map_err(AppError::from)?;
    agent
        .sender
        .send(Message::Text(message))
        .await
        .map_err(|_| AppError::bad_request("agent unavailable"))?;
    Ok(StatusCode::OK)
}

async fn agent_ws(
    State(state): State<AppState>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> impl IntoResponse {
    let Some(registration_token) = extract_agent_registration_token(&headers) else {
        return (StatusCode::UNAUTHORIZED, "unauthorized").into_response();
    };

    ws.on_upgrade(move |socket| handle_socket(socket, state, registration_token))
}

async fn handle_socket(socket: WebSocket, state: AppState, registration_token: String) {
    let (mut ws_sender, mut ws_receiver) = socket.split();
    let (tx, mut rx) = mpsc::channel::<Message>(32);
    let sender = tx.clone();

    let send_task = tokio::spawn(async move {
        while let Some(message) = rx.recv().await {
            if ws_sender.send(message).await.is_err() {
                break;
            }
        }
    });

    let mut ping_interval =
        tokio::time::interval(Duration::from_secs(state.config.ping_interval_secs));
    let mut agent_id: Option<String> = None;

    loop {
        tokio::select! {
            _ = ping_interval.tick() => {
                if sender.send(Message::Ping(Vec::new())).await.is_err() {
                    break;
                }
            }
            message = ws_receiver.next() => {
                match message {
                    Some(Ok(Message::Text(text))) => {
                        if let Err(err) = handle_agent_message(&state, &registration_token, &text, &sender, &mut agent_id).await {
                            warn!(error = ?err, "failed to handle agent message");
                            break;
                        }
                    }
                    Some(Ok(Message::Binary(bytes))) => {
                        if let Ok(text) = String::from_utf8(bytes) {
                            if let Err(err) = handle_agent_message(&state, &registration_token, &text, &sender, &mut agent_id).await {
                                warn!(error = ?err, "failed to handle agent message");
                                break;
                            }
                        }
                    }
                    Some(Ok(Message::Close(_))) => break,
                    Some(Ok(_)) => {}
                    Some(Err(err)) => {
                        warn!(error = ?err, "websocket error");
                        break;
                    }
                    None => break,
                }
            }
        }
    }

    if let Some(agent_id) = agent_id {
        if !state
            .agents
            .disconnect_unless_replaced(&agent_id, &sender)
            .await
        {
            send_task.abort();
            return;
        }
        if let Err(err) =
            report_agent_connection_status(&state, &agent_id, None, "disconnected", None).await
        {
            warn!(agent_id = %agent_id, error = %err, "failed to report agent websocket disconnection");
        }

        let stale_shell_ids = {
            let sessions = state.active_shell_sessions.read().await;
            sessions
                .iter()
                .filter_map(|(session_id, session)| {
                    if session.agent_id == agent_id {
                        Some(session_id.clone())
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>()
        };
        if !stale_shell_ids.is_empty() {
            let mut sessions = state.active_shell_sessions.write().await;
            for session_id in stale_shell_ids {
                sessions.remove(&session_id);
            }
        }

        let stale_file_transfer_ids = {
            let sessions = state.file_transfer_sessions.read().await;
            sessions
                .iter()
                .filter_map(|(session_id, session)| {
                    if session.agent_id == agent_id {
                        Some(session_id.clone())
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>()
        };
        if !stale_file_transfer_ids.is_empty() {
            let mut sessions = state.file_transfer_sessions.write().await;
            for session_id in stale_file_transfer_ids {
                sessions.remove(&session_id);
            }
        }

        let stale_remote_desktop_ids = {
            let sessions = state.remote_desktop_sessions.read().await;
            sessions
                .iter()
                .filter_map(|(session_id, session)| {
                    if session.agent_id == agent_id {
                        Some(session_id.clone())
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>()
        };
        if !stale_remote_desktop_ids.is_empty() {
            let mut sessions = state.remote_desktop_sessions.write().await;
            for session_id in stale_remote_desktop_ids {
                sessions.remove(&session_id);
            }
        }

        let stale_remote_registry_ids = {
            let sessions = state.remote_registry_sessions.read().await;
            sessions
                .iter()
                .filter_map(|(session_id, session)| {
                    if session.agent_id == agent_id {
                        Some(session_id.clone())
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>()
        };
        if !stale_remote_registry_ids.is_empty() {
            let mut sessions = state.remote_registry_sessions.write().await;
            for session_id in stale_remote_registry_ids {
                sessions.remove(&session_id);
            }
        }

        let stale_chat_ids = {
            let sessions = state.chat_sessions.read().await;
            sessions
                .iter()
                .filter_map(|(session_id, session)| {
                    if session.agent_id == agent_id {
                        Some(session_id.clone())
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>()
        };
        if !stale_chat_ids.is_empty() {
            let mut sessions = state.chat_sessions.write().await;
            for session_id in stale_chat_ids {
                sessions.remove(&session_id);
            }
        }
    }

    send_task.abort();
}

async fn handle_agent_message(
    state: &AppState,
    registration_token: &str,
    text: &str,
    sender: &mpsc::Sender<Message>,
    agent_id: &mut Option<String>,
) -> Result<()> {
    let envelope: IncomingEnvelope =
        serde_json::from_str(text).context("parse message envelope")?;

    match envelope.message_type.as_str() {
        "agent_hello" => {
            let payload: AgentHello =
                serde_json::from_value(envelope.data).context("parse agent_hello")?;
            let agent_id_value = payload.agent_id.clone();
            let host = normalize_host(&payload.ip);
            let is_admin = payload.is_admin;
            let local_addrs = payload.local_addrs.clone();
            let hostname = normalize_text_value(Some(payload.hostname.clone()));
            let os = normalize_text_value(Some(payload.os.clone()));
            let version = normalize_text_value(payload.version.clone());
            let platform = normalize_agent_platform(payload.platform, os.as_deref());
            let features = if payload.features == AgentFeatureCapabilities::default()
                && platform != AgentPlatform::Windows
            {
                features_for_platform(platform)
            } else {
                payload.features.clone()
            };
            let enrollment = enroll_agent(
                state,
                registration_token,
                &agent_id_value,
                &payload.hostname,
                &payload.os,
                &payload.ip,
                payload.version.as_deref(),
            )
            .await?;
            upsert_device(state, payload, &enrollment.organization_id).await?;
            if let Err(err) = report_agent_connection_status(
                state,
                &agent_id_value,
                Some(&enrollment.organization_id),
                "connected",
                version.as_deref(),
            )
            .await
            {
                warn!(agent_id = %agent_id_value, error = %err, "failed to report agent websocket connection");
            }
            register_agent(
                state,
                sender.clone(),
                &agent_id_value,
                host,
                local_addrs,
                Some(is_admin),
                hostname,
                os,
                version,
                platform,
                features,
            )
            .await;
            *agent_id = Some(agent_id_value);
        }
        "inventory_update" => {
            let payload: InventoryUpdate =
                serde_json::from_value(envelope.data).context("parse inventory_update")?;
            let agent_id_value = payload.agent_id.clone();
            let host = normalize_host(&payload.ip);
            let hostname = normalize_text_value(Some(payload.hostname.clone()));
            let os = normalize_text_value(Some(payload.os.clone()));
            let version = normalize_text_value(payload.version.clone());
            let platform = normalize_agent_platform(AgentPlatform::Unknown, os.as_deref());
            let features = features_for_platform(platform);
            let enrollment = enroll_agent(
                state,
                registration_token,
                &agent_id_value,
                &payload.hostname,
                &payload.os,
                &payload.ip,
                payload.version.as_deref(),
            )
            .await?;
            upsert_inventory(state, payload, &enrollment.organization_id).await?;
            if let Err(err) = report_agent_connection_status(
                state,
                &agent_id_value,
                Some(&enrollment.organization_id),
                "connected",
                version.as_deref(),
            )
            .await
            {
                warn!(agent_id = %agent_id_value, error = %err, "failed to report agent websocket connection");
            }
            register_agent(
                state,
                sender.clone(),
                &agent_id_value,
                host,
                None,
                None,
                hostname,
                os,
                version,
                platform,
                features,
            )
            .await;
            *agent_id = Some(agent_id_value);
        }
        "full_snapshot" => {
            let payload: FullSnapshotUpdate =
                serde_json::from_value(envelope.data).context("parse full_snapshot")?;
            let agent_id_value = payload.agent_id.clone();
            upsert_full_snapshot(state, payload).await?;
            *agent_id = Some(agent_id_value);
        }
        "telemetry_events" => {
            let payload: TelemetryEventsUpdate =
                serde_json::from_value(envelope.data).context("parse telemetry_events")?;
            let agent_id_value = payload.agent_id.clone();
            upsert_telemetry_events(state, payload).await?;
            *agent_id = Some(agent_id_value);
        }
        "patch_jobs_poll" => {
            let payload: PatchJobsPollPayload =
                serde_json::from_value(envelope.data).context("parse patch_jobs_poll")?;
            let Some(agent_id_value) = agent_id.clone() else {
                warn!("patch job poll received before agent registration");
                return Ok(());
            };
            let jobs = match claim_patch_jobs_for_agent(
                state,
                &agent_id_value,
                payload.limit.unwrap_or(1),
            )
            .await
            {
                Ok(jobs) => jobs,
                Err(error) => {
                    warn!(
                        agent_id = %agent_id_value,
                        error = ?error,
                        "patch job claim failed"
                    );
                    Vec::new()
                }
            };
            let response = OutgoingEnvelope {
                message_type: "patch_jobs",
                data: PatchJobsResponsePayload {
                    request_id: payload.request_id,
                    jobs,
                },
            };
            let text = serde_json::to_string(&response).context("serialize patch_jobs")?;
            sender
                .send(Message::Text(text))
                .await
                .context("send patch_jobs")?;
        }
        "patch_state_checkin" => {
            let payload: PatchStateCheckinPayload =
                serde_json::from_value(envelope.data).context("parse patch_state_checkin")?;
            let request_id = payload.request_id.clone();
            let Some(agent_id_value) = agent_id.clone() else {
                warn!("patch state check-in received before agent registration");
                return Ok(());
            };
            let plan = match evaluate_patch_state_for_agent(state, &agent_id_value, payload).await {
                Ok(plan) => plan,
                Err(error) => {
                    warn!(
                        agent_id = %agent_id_value,
                        error = ?error,
                        "patch state check-in evaluation failed"
                    );
                    serde_json::json!({
                        "schemaVersion": 1,
                        "generatedAt": Utc::now().to_rfc3339(),
                        "agentId": agent_id_value,
                        "managedMode": false,
                        "nativeWindowsUpdateControl": false,
                        "actions": []
                    })
                }
            };
            let response = OutgoingEnvelope {
                message_type: "patch_action_plan",
                data: PatchActionPlanPayload { request_id, plan },
            };
            let text = serde_json::to_string(&response).context("serialize patch_action_plan")?;
            sender
                .send(Message::Text(text))
                .await
                .context("send patch_action_plan")?;
        }
        "patch_action_result" => {
            let payload: PatchActionResultPayload =
                serde_json::from_value(envelope.data).context("parse patch_action_result")?;
            let Some(agent_id_value) = agent_id.clone() else {
                warn!("patch action result received before agent registration");
                return Ok(());
            };
            if let Err(error) =
                report_patch_action_result_for_agent(state, &agent_id_value, payload).await
            {
                warn!(
                    agent_id = %agent_id_value,
                    error = ?error,
                    "patch action result report failed"
                );
            }
        }
        "patch_job_update" => {
            let payload: PatchJobUpdatePayload =
                serde_json::from_value(envelope.data).context("parse patch_job_update")?;
            let Some(agent_id_value) = agent_id.clone() else {
                warn!("patch job update received before agent registration");
                return Ok(());
            };
            if let Err(error) =
                report_patch_job_update_for_agent(state, &agent_id_value, payload).await
            {
                warn!(
                    agent_id = %agent_id_value,
                    error = ?error,
                    "patch job status report failed"
                );
            }
        }
        "patch_job_progress" => {
            let payload: PatchJobProgressPayload =
                serde_json::from_value(envelope.data).context("parse patch_job_progress")?;
            let Some(agent_id_value) = agent_id.clone() else {
                warn!("patch job progress received before agent registration");
                return Ok(());
            };
            if let Err(error) =
                publish_patch_progress_for_agent(state, &agent_id_value, payload).await
            {
                warn!(
                    agent_id = %agent_id_value,
                    error = ?error,
                    "patch progress publish failed"
                );
            }
        }
        "macos_update_account_status" => {
            let payload: MacosUpdateAccountStatusPayload = serde_json::from_value(envelope.data)
                .context("parse macos_update_account_status")?;
            let Some(agent_id_value) = agent_id.clone() else {
                warn!("macOS update account status received before agent registration");
                return Ok(());
            };
            if let Err(error) =
                report_macos_update_account_status_for_agent(state, &agent_id_value, payload).await
            {
                warn!(
                    agent_id = %agent_id_value,
                    error = ?error,
                    "macOS update account status report failed"
                );
            }
        }
        "feature_upgrade_preflight_jobs_poll" => {
            let payload: FeatureUpgradePreflightJobsPollPayload =
                serde_json::from_value(envelope.data)
                    .context("parse feature_upgrade_preflight_jobs_poll")?;
            let Some(agent_id_value) = agent_id.clone() else {
                warn!("feature upgrade preflight poll received before agent registration");
                return Ok(());
            };
            let jobs = match claim_feature_upgrade_preflight_jobs_for_agent(
                state,
                &agent_id_value,
                payload.limit.unwrap_or(1),
            )
            .await
            {
                Ok(jobs) => jobs,
                Err(error) => {
                    warn!(
                        agent_id = %agent_id_value,
                        error = ?error,
                        "feature upgrade preflight job claim failed"
                    );
                    Vec::new()
                }
            };
            let response = OutgoingEnvelope {
                message_type: "feature_upgrade_preflight_jobs",
                data: FeatureUpgradePreflightJobsResponsePayload {
                    request_id: payload.request_id,
                    jobs,
                },
            };
            let text = serde_json::to_string(&response)
                .context("serialize feature_upgrade_preflight_jobs")?;
            sender
                .send(Message::Text(text))
                .await
                .context("send feature_upgrade_preflight_jobs")?;
        }
        "feature_upgrade_preflight_progress" => {
            let payload: FeatureUpgradePreflightProgressPayload =
                serde_json::from_value(envelope.data)
                    .context("parse feature_upgrade_preflight_progress")?;
            let Some(agent_id_value) = agent_id.clone() else {
                warn!("feature upgrade preflight progress received before agent registration");
                return Ok(());
            };
            if let Err(error) = publish_feature_upgrade_preflight_progress_for_agent(
                state,
                &agent_id_value,
                payload,
            )
            .await
            {
                warn!(
                    agent_id = %agent_id_value,
                    error = ?error,
                    "feature upgrade preflight progress publish failed"
                );
            }
        }
        "feature_upgrade_stage_iso_jobs_poll" => {
            let payload: FeatureUpgradeStageIsoJobsPollPayload =
                serde_json::from_value(envelope.data)
                    .context("parse feature_upgrade_stage_iso_jobs_poll")?;
            let Some(agent_id_value) = agent_id.clone() else {
                warn!("feature upgrade stage ISO poll received before agent registration");
                return Ok(());
            };
            let jobs = match claim_feature_upgrade_stage_iso_jobs_for_agent(
                state,
                &agent_id_value,
                payload.limit.unwrap_or(1),
            )
            .await
            {
                Ok(jobs) => jobs,
                Err(error) => {
                    warn!(
                        agent_id = %agent_id_value,
                        error = ?error,
                        "feature upgrade stage ISO job claim failed"
                    );
                    Vec::new()
                }
            };
            let response = OutgoingEnvelope {
                message_type: "feature_upgrade_stage_iso_jobs",
                data: FeatureUpgradeStageIsoJobsResponsePayload {
                    request_id: payload.request_id,
                    jobs,
                },
            };
            let text = serde_json::to_string(&response)
                .context("serialize feature_upgrade_stage_iso_jobs")?;
            sender
                .send(Message::Text(text))
                .await
                .context("send feature_upgrade_stage_iso_jobs")?;
        }
        "feature_upgrade_stage_iso_progress" => {
            let payload: FeatureUpgradeStageIsoProgressPayload =
                serde_json::from_value(envelope.data)
                    .context("parse feature_upgrade_stage_iso_progress")?;
            let Some(agent_id_value) = agent_id.clone() else {
                warn!("feature upgrade stage ISO progress received before agent registration");
                return Ok(());
            };
            if let Err(error) = publish_feature_upgrade_stage_iso_progress_for_agent(
                state,
                &agent_id_value,
                payload,
            )
            .await
            {
                warn!(
                    agent_id = %agent_id_value,
                    error = ?error,
                    "feature upgrade stage ISO progress publish failed"
                );
            }
        }
        "feature_upgrade_start_jobs_poll" => {
            let payload: FeatureUpgradeStartJobsPollPayload = serde_json::from_value(envelope.data)
                .context("parse feature_upgrade_start_jobs_poll")?;
            let Some(agent_id_value) = agent_id.clone() else {
                warn!("feature upgrade start poll received before agent registration");
                return Ok(());
            };
            let jobs = match claim_feature_upgrade_start_jobs_for_agent(
                state,
                &agent_id_value,
                payload.limit.unwrap_or(1),
            )
            .await
            {
                Ok(jobs) => jobs,
                Err(error) => {
                    warn!(
                        agent_id = %agent_id_value,
                        error = ?error,
                        "feature upgrade start job claim failed"
                    );
                    Vec::new()
                }
            };
            let response = OutgoingEnvelope {
                message_type: "feature_upgrade_start_jobs",
                data: FeatureUpgradeStartJobsResponsePayload {
                    request_id: payload.request_id,
                    jobs,
                },
            };
            let text =
                serde_json::to_string(&response).context("serialize feature_upgrade_start_jobs")?;
            sender
                .send(Message::Text(text))
                .await
                .context("send feature_upgrade_start_jobs")?;
        }
        "feature_upgrade_start_progress" => {
            let payload: FeatureUpgradeStartProgressPayload = serde_json::from_value(envelope.data)
                .context("parse feature_upgrade_start_progress")?;
            let Some(agent_id_value) = agent_id.clone() else {
                warn!("feature upgrade start progress received before agent registration");
                return Ok(());
            };
            if let Err(error) =
                publish_feature_upgrade_start_progress_for_agent(state, &agent_id_value, payload)
                    .await
            {
                warn!(
                    agent_id = %agent_id_value,
                    error = ?error,
                    "feature upgrade start progress publish failed"
                );
            }
        }
        "remediation_jobs_poll" => {
            let payload: RemediationJobsPollPayload =
                serde_json::from_value(envelope.data).context("parse remediation_jobs_poll")?;
            let Some(agent_id_value) = agent_id.clone() else {
                warn!("remediation job poll received before agent registration");
                return Ok(());
            };
            let jobs = match claim_remediation_jobs_for_agent(
                state,
                &agent_id_value,
                payload.limit.unwrap_or(1),
            )
            .await
            {
                Ok(jobs) => jobs,
                Err(error) => {
                    warn!(
                        agent_id = %agent_id_value,
                        error = ?error,
                        "remediation job claim failed"
                    );
                    Vec::new()
                }
            };
            let response = OutgoingEnvelope {
                message_type: "remediation_jobs",
                data: RemediationJobsResponsePayload {
                    request_id: payload.request_id,
                    jobs,
                },
            };
            let text = serde_json::to_string(&response).context("serialize remediation_jobs")?;
            sender
                .send(Message::Text(text))
                .await
                .context("send remediation_jobs")?;
        }
        "remediation_job_update" => {
            let payload: RemediationJobUpdatePayload =
                serde_json::from_value(envelope.data).context("parse remediation_job_update")?;
            let Some(agent_id_value) = agent_id.clone() else {
                warn!("remediation job update received before agent registration");
                return Ok(());
            };
            if let Err(error) =
                publish_remediation_status_for_agent(state, &agent_id_value, payload).await
            {
                warn!(
                    agent_id = %agent_id_value,
                    error = ?error,
                    "remediation job status publish failed"
                );
            }
        }
        "linux_shell_credential" => {
            let payload: LinuxShellCredentialPayload =
                serde_json::from_value(envelope.data).context("parse linux_shell_credential")?;
            let stored = store_linux_shell_credential(state, &payload).await?;
            if !stored.accepted {
                return Err(anyhow::anyhow!("api did not accept Linux shell credential"));
            }
            let response = OutgoingEnvelope {
                message_type: "linux_shell_credential_stored",
                data: LinuxShellCredentialStoredPayload {
                    credential_id: stored.credential_id,
                    stored_at: stored.stored_at,
                },
            };
            let text = serde_json::to_string(&response)
                .context("serialize linux_shell_credential_stored")?;
            sender
                .send(Message::Text(text))
                .await
                .context("send linux_shell_credential_stored")?;
            *agent_id = Some(payload.agent_id);
        }
        "device_details" => {
            let payload: DeviceDetailsPayload =
                serde_json::from_value(envelope.data).context("parse device_details")?;
            handle_device_details(state, payload).await;
        }
        "shell_offer" => {
            let payload: ShellOfferPayload =
                serde_json::from_value(envelope.data).context("parse shell_offer")?;
            handle_shell_offer(state, payload).await;
        }
        "shell_error" => {
            let payload: ShellErrorPayload =
                serde_json::from_value(envelope.data).context("parse shell_error")?;
            handle_shell_error(state, payload).await;
        }
        "shell_output" => {
            let payload: ShellOutputPayload =
                serde_json::from_value(envelope.data).context("parse shell_output")?;
            handle_shell_output(state, payload).await;
        }
        "rdp_sessions_response" => {
            let payload: RdpSessionsResponsePayload =
                serde_json::from_value(envelope.data).context("parse rdp_sessions_response")?;
            handle_rdp_sessions_response(state, payload).await;
        }
        "session_capabilities_response" => {
            let payload: SessionCapabilitiesResponse = serde_json::from_value(envelope.data)
                .context("parse session_capabilities_response")?;
            handle_session_capabilities_response(state, payload).await;
        }
        "quic_reflex" => {
            let payload: QuicReflexPayload =
                serde_json::from_value(envelope.data).context("parse quic_reflex")?;
            handle_quic_reflex(state, payload).await;
        }
        "remote_desktop_unavailable" => {
            let payload: RemoteDesktopUnavailablePayload = serde_json::from_value(envelope.data)
                .context("parse remote_desktop_unavailable")?;
            handle_remote_desktop_unavailable(state, payload).await;
        }
        unknown => {
            warn!(message_type = unknown, "unknown message type");
        }
    }

    Ok(())
}

async fn upsert_device(state: &AppState, payload: AgentHello, organization_id: &str) -> Result<()> {
    let body = serde_json::json!({
        "agentId": payload.agent_id,
        "organizationId": organization_id,
        "hostname": payload.hostname,
        "os": payload.os,
        "ip": payload.ip,
        "version": payload.version,
    });

    let _: ApiDeviceSummary = api_request(
        state,
        reqwest::Method::POST,
        "/rmm/devices",
        None,
        Some(body),
        true,
        "upsert device",
    )
    .await?;

    Ok(())
}

async fn report_agent_connection_status(
    state: &AppState,
    agent_id: &str,
    organization_id: Option<&str>,
    status: &str,
    version: Option<&str>,
) -> Result<()> {
    let path = format!("/rmm/devices/{agent_id}/connection-status");
    let body = serde_json::json!({
        "organizationId": organization_id,
        "status": status,
        "observedAt": Utc::now(),
        "version": version,
    });

    let _: Value = api_request(
        state,
        reqwest::Method::POST,
        &path,
        None,
        Some(body),
        true,
        "report agent connection status",
    )
    .await?;

    Ok(())
}

async fn upsert_inventory(
    state: &AppState,
    payload: InventoryUpdate,
    organization_id: &str,
) -> Result<()> {
    let path = format!("/rmm/devices/{}/inventory", payload.agent_id);
    let body = serde_json::json!({
        "agentId": payload.agent_id,
        "organizationId": organization_id,
        "hostname": payload.hostname,
        "os": payload.os,
        "ip": payload.ip,
        "version": payload.version,
        "inventory": payload.inventory,
    });

    let _: ApiDeviceSummary = api_request(
        state,
        reqwest::Method::POST,
        &path,
        None,
        Some(body),
        true,
        "upsert inventory",
    )
    .await?;

    Ok(())
}

async fn upsert_full_snapshot(state: &AppState, payload: FullSnapshotUpdate) -> Result<()> {
    let device_scope = fetch_device_scope(state, &payload.agent_id).await?;
    let body = serde_json::json!({
        "organizationId": device_scope.organization_id,
        "agentId": payload.agent_id,
        "collectedAt": payload.collected_at,
        "snapshot": payload.snapshot,
        "snapshotRequestId": payload.snapshot_request_id,
    });

    let url = full_snapshot_ingest_url(&state.config);
    let response: ApiAccepted = api_request_with_url(
        state,
        reqwest::Method::POST,
        &url,
        None,
        Some(body.clone()),
        true,
        "enqueue full snapshot",
    )
    .await?;

    if !response.accepted {
        return Err(AppError::internal("telemetry api did not accept snapshot").into());
    }

    if state.config.telemetry_producer_url.is_some() {
        let compat_url = api_url(&state.config, "rmm/telemetry/snapshots/upsert");
        let response: ApiAccepted = api_request_with_url(
            state,
            reqwest::Method::POST,
            &compat_url,
            None,
            Some(body),
            true,
            "project full snapshot read model",
        )
        .await?;

        if !response.accepted {
            return Err(AppError::internal("telemetry api did not project snapshot").into());
        }
    }

    Ok(())
}

async fn upsert_telemetry_events(state: &AppState, payload: TelemetryEventsUpdate) -> Result<()> {
    let device_scope = fetch_device_scope(state, &payload.agent_id).await?;
    let body = serde_json::json!({
        "organizationId": device_scope.organization_id,
        "agentId": payload.agent_id,
        "events": payload.events,
    });

    let url = telemetry_url(&state.config, "events");
    let response: ApiAccepted = api_request_with_url(
        state,
        reqwest::Method::POST,
        &url,
        None,
        Some(body),
        true,
        "enqueue telemetry events",
    )
    .await?;

    if !response.accepted {
        return Err(AppError::internal("telemetry api did not accept events").into());
    }

    Ok(())
}

async fn store_linux_shell_credential(
    state: &AppState,
    payload: &LinuxShellCredentialPayload,
) -> Result<ApiCredentialStoreResponse> {
    let body = serde_json::json!({
        "username": payload.username,
        "password": payload.password,
        "credentialId": payload.credential_id,
        "version": payload.version,
        "generatedAt": payload.generated_at,
    });

    Ok(api_request(
        state,
        reqwest::Method::POST,
        &format!("/rmm/devices/{}/linux-shell-credential", payload.agent_id),
        None,
        Some(body),
        true,
        "store Linux shell credential",
    )
    .await?)
}

async fn update_device_details(
    state: &AppState,
    agent_id: &str,
    details: &Value,
) -> Result<DeviceSummary> {
    let path = format!("/rmm/devices/{agent_id}");
    let body = serde_json::json!({
        "deviceDetails": details,
    });

    let mut updated: ApiDeviceSummary = api_request(
        state,
        reqwest::Method::PATCH,
        &path,
        None,
        Some(body),
        true,
        "update device details",
    )
    .await?;
    updated.device_details = Some(details.clone());

    Ok(updated.into())
}

async fn register_agent(
    state: &AppState,
    sender: mpsc::Sender<Message>,
    agent_id: &str,
    host: Option<String>,
    local_addrs: Option<Vec<LocalAddr>>,
    is_admin: Option<bool>,
    hostname: Option<String>,
    os: Option<String>,
    version: Option<String>,
    platform: AgentPlatform,
    features: AgentFeatureCapabilities,
) {
    state
        .agents
        .register(
            agent_id,
            AgentRegistration {
                sender,
                host,
                local_addrs,
                is_admin,
                hostname,
                os,
                version,
                platform,
                features,
            },
        )
        .await;
}

async fn handle_shell_offer(state: &AppState, payload: ShellOfferPayload) {
    let mut sessions = state.shell_sessions.write().await;
    if let Some(session) = sessions.remove(&payload.session_id) {
        let _ = session.response_tx.send(Ok(payload));
    }
}

async fn handle_shell_error(state: &AppState, payload: ShellErrorPayload) {
    let mut sessions = state.shell_sessions.write().await;
    if let Some(session) = sessions.remove(&payload.session_id) {
        let _ = session.response_tx.send(Err(payload.error));
    }
}

async fn handle_device_details(state: &AppState, payload: DeviceDetailsPayload) {
    let mut requests = state.detail_requests.write().await;
    if let Some(request) = requests.remove(&payload.request_id) {
        let _ = request.response_tx.send(payload.details);
    }
}

async fn handle_shell_output(state: &AppState, payload: ShellOutputPayload) {
    let mut commands = state.shell_commands.write().await;
    if let Some(command) = commands.remove(&payload.request_id) {
        let _ = command.response_tx.send(payload);
    }
}

async fn handle_rdp_sessions_response(state: &AppState, payload: RdpSessionsResponsePayload) {
    let mut requests = state.rdp_sessions_requests.write().await;
    if let Some(request) = requests.remove(&payload.request_id) {
        let _ = request.response_tx.send(payload.sessions);
    }
}

async fn handle_session_capabilities_response(
    state: &AppState,
    payload: SessionCapabilitiesResponse,
) {
    let mut requests = state.capability_requests.write().await;
    if let Some(request) = requests.remove(&payload.request_id) {
        let _ = request.response_tx.send(payload.capabilities);
    }
}

async fn handle_quic_reflex(state: &AppState, payload: QuicReflexPayload) {
    let mut requests = state.quic_reflex_requests.write().await;
    if let Some(request) = requests.remove(&payload.session_id) {
        let _ = request
            .response_tx
            .send(QuicReflexResult::Success(payload.reflex));
    }
}

async fn handle_remote_desktop_unavailable(
    state: &AppState,
    payload: RemoteDesktopUnavailablePayload,
) {
    warn!(
        session_id = %payload.session_id,
        reason = %payload.reason,
        message = ?payload.message,
        "remote_desktop_unavailable from agent"
    );
    let mut requests = state.quic_reflex_requests.write().await;
    if let Some(request) = requests.remove(&payload.session_id) {
        let _ = request
            .response_tx
            .send(QuicReflexResult::DisplayUnavailable {
                reason: payload.reason,
                message: payload.message,
            });
    }
}

fn normalize_host(host: &str) -> Option<String> {
    let trimmed = host.trim();
    if trimmed.is_empty() || trimmed == "unknown" {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn normalize_text_value(value: Option<String>) -> Option<String> {
    let Some(raw) = value else {
        return None;
    };
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("unknown") {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn extract_api_base(headers: &HeaderMap) -> Option<String> {
    let host = headers
        .get("x-forwarded-host")
        .or_else(|| headers.get("host"))
        .and_then(|value| value.to_str().ok())?;
    let proto = headers
        .get("x-forwarded-proto")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("http");
    Some(format!("{proto}://{host}"))
}

fn build_connect_url(
    host: &str,
    port: u16,
    session_id: &str,
    token: &str,
    agent_id: &str,
    api_base: Option<&str>,
    mode: Option<&str>,
    run_as: Option<ShellRunAs>,
    target_session_id: Option<u32>,
    system_supported: bool,
    relay_url: Option<&str>,
    e2e_key: Option<&str>,
) -> String {
    let mut url = format!(
        "rmm://connect?host={host}&port={port}&session={session_id}&token={token}&agent={agent_id}"
    );
    if let Some(mode) = mode {
        url.push_str(&format!("&mode={mode}"));
    }
    if let Some(run_as) = run_as {
        let run_as_value = match run_as {
            ShellRunAs::User => "user",
            ShellRunAs::System => "system",
        };
        url.push_str(&format!("&runAs={run_as_value}"));
    }
    if let Some(target_session_id) = target_session_id {
        url.push_str(&format!("&targetSessionId={target_session_id}"));
    }
    if system_supported {
        url.push_str("&system=1");
    }
    if let Some(api) = api_base {
        url.push_str(&format!("&api={api}"));
    }
    if let Some(relay_url) = relay_url {
        url.push_str(&format!("&relayUrl={relay_url}"));
    }
    if let Some(e2e_key) = e2e_key {
        url.push_str(&format!("&e2eKey={e2e_key}"));
    }
    url
}

fn build_desktop_connect_url_with_mode(
    session_id: &str,
    token: &str,
    agent_id: &str,
    api_base: Option<&str>,
    mode: &str,
) -> String {
    let mut url =
        format!("rmm://connect?session={session_id}&token={token}&agent={agent_id}&mode={mode}");
    if let Some(api) = api_base {
        url.push_str(&format!("&api={api}"));
    }
    url
}

fn build_file_transfer_connect_url(
    session_id: &str,
    token: &str,
    agent_id: &str,
    api_base: Option<&str>,
) -> String {
    let mut url = format!(
        "rmm://connect?session={session_id}&token={token}&agent={agent_id}&mode=file-transfer"
    );
    if let Some(api) = api_base {
        url.push_str(&format!("&api={api}"));
    }
    url
}

fn build_registry_connect_url(
    session_id: &str,
    token: &str,
    agent_id: &str,
    api_base: Option<&str>,
) -> String {
    let mut url =
        format!("rmm://connect?session={session_id}&token={token}&agent={agent_id}&mode=registry");
    if let Some(api) = api_base {
        url.push_str(&format!("&api={api}"));
    }
    url
}

fn build_chat_connect_url(
    session_id: &str,
    token: &str,
    agent_id: &str,
    api_base: Option<&str>,
) -> String {
    let mut url =
        format!("rmm://connect?session={session_id}&token={token}&agent={agent_id}&mode=chat");
    if let Some(api) = api_base {
        url.push_str(&format!("&api={api}"));
    }
    url
}

fn extract_agent_registration_token(headers: &HeaderMap) -> Option<String> {
    if let Some(value) = extract_bearer(headers) {
        let token = value.trim();
        if !token.is_empty() {
            return Some(token.to_string());
        }
    }

    if let Some(value) = headers
        .get("x-rmm-token")
        .and_then(|value| value.to_str().ok())
    {
        let token = value.trim();
        if !token.is_empty() {
            return Some(token.to_string());
        }
    }

    None
}

fn extract_bearer(headers: &HeaderMap) -> Option<String> {
    let value = headers.get(axum::http::header::AUTHORIZATION)?;
    let value = value.to_str().ok()?;
    let mut parts = value.splitn(2, ' ');
    let scheme = parts.next()?.trim();
    let token = parts.next()?.trim();
    if scheme.eq_ignore_ascii_case("bearer") && !token.is_empty() {
        Some(token.to_string())
    } else {
        None
    }
}

fn api_url(config: &Config, path: &str) -> String {
    let base = config.api_base_url.trim_end_matches('/');
    let suffix = path.trim_start_matches('/');
    format!("{base}/{suffix}")
}

/// Base URL and path for telemetry ingestion. When RMM_TELEMETRY_PRODUCER_URL is set, use producer paths (/telemetry/snapshots, /telemetry/events); otherwise use API backend paths (/rmm/telemetry/...).
fn telemetry_url(config: &Config, path_suffix: &str) -> String {
    match &config.telemetry_producer_url {
        Some(base) => {
            let base = base.trim_end_matches('/');
            format!("{base}/telemetry/{path_suffix}")
        }
        None => api_url(config, &format!("rmm/telemetry/{path_suffix}")),
    }
}

fn full_snapshot_ingest_url(config: &Config) -> String {
    match &config.telemetry_producer_url {
        Some(_) => telemetry_url(config, "snapshots"),
        None => api_url(config, "rmm/telemetry/snapshots/upsert"),
    }
}

async fn api_request<T: DeserializeOwned>(
    state: &AppState,
    method: reqwest::Method,
    path: &str,
    bearer: Option<&str>,
    body: Option<Value>,
    use_server_key: bool,
    context: &str,
) -> Result<T, AppError> {
    let url = api_url(&state.config, path);
    api_request_with_url(state, method, &url, bearer, body, use_server_key, context).await
}

async fn api_request_with_url<T: DeserializeOwned>(
    state: &AppState,
    method: reqwest::Method,
    url: &str,
    bearer: Option<&str>,
    body: Option<Value>,
    use_server_key: bool,
    context: &str,
) -> Result<T, AppError> {
    let mut request = state.api_client.request(method.clone(), url);
    if let Some(token) = bearer {
        request = request.bearer_auth(token);
    }
    if use_server_key {
        let Some(key) = state
            .config
            .talos_server_api_key
            .as_ref()
            .filter(|value| !value.is_empty())
        else {
            return Err(AppError::internal("RMM_SERVER_API_KEY must be configured"));
        };
        request = request.header("x-rmm-server-key", key);
    }
    if let Some(body) = body {
        request = request.json(&body);
    }
    let response = request
        .send()
        .await
        .map_err(|err| AppError::internal(&format!("api request failed: {err}")))?;

    if response.status().is_success() {
        return response
            .json::<T>()
            .await
            .map_err(|err| AppError::internal(&format!("api parse failed: {err}")));
    }

    let status = StatusCode::from_u16(response.status().as_u16())
        .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    let message = response
        .text()
        .await
        .unwrap_or_else(|_| context.to_string());
    Err(AppError { status, message })
}

fn cors_layer(config: &Config) -> CorsLayer {
    if config
        .cors_origins
        .iter()
        .any(|origin| origin.trim() == "*")
    {
        CorsLayer::new()
            .allow_origin(Any)
            .allow_headers(Any)
            .allow_methods(Any)
    } else if config.cors_origins.is_empty() {
        CorsLayer::new()
            .allow_origin(Any)
            .allow_headers(Any)
            .allow_methods(Any)
    } else {
        let origins = config
            .cors_origins
            .iter()
            .filter_map(|origin| origin.parse().ok())
            .collect::<Vec<_>>();
        CorsLayer::new()
            .allow_origin(origins)
            .allow_headers(Any)
            .allow_methods(Any)
    }
}

#[derive(Debug)]
struct AppError {
    status: StatusCode,
    message: String,
}

impl AppError {
    fn not_found(message: &str) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            message: message.to_string(),
        }
    }

    fn unauthorized(message: &str) -> Self {
        Self {
            status: StatusCode::UNAUTHORIZED,
            message: message.to_string(),
        }
    }

    fn bad_request(message: &str) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            message: message.to_string(),
        }
    }

    fn forbidden(message: &str) -> Self {
        Self {
            status: StatusCode::FORBIDDEN,
            message: message.to_string(),
        }
    }

    fn timeout(message: &str) -> Self {
        Self {
            status: StatusCode::GATEWAY_TIMEOUT,
            message: message.to_string(),
        }
    }

    fn service_unavailable(message: &str) -> Self {
        Self {
            status: StatusCode::SERVICE_UNAVAILABLE,
            message: message.to_string(),
        }
    }

    fn gone(message: &str) -> Self {
        Self {
            status: StatusCode::GONE,
            message: message.to_string(),
        }
    }

    fn internal(message: &str) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: message.to_string(),
        }
    }
}

impl std::fmt::Display for AppError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for AppError {}

impl From<anyhow::Error> for AppError {
    fn from(error: anyhow::Error) -> Self {
        error!(error = ?error, "talos_server error");
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: "internal server error".to_string(),
        }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> axum::response::Response {
        (self.status, self.message).into_response()
    }
}
