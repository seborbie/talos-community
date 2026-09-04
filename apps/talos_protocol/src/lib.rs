//! Shared protocol types for RMM agent, server, and viewer.
//!
//! # Talos tracing severity (shipped Windows binaries)
//!
//! Binaries that call [`rmm_tracing_filter_directive`] should follow this rubric:
//!
//! | Level | Use for | Avoid |
//! |-------|---------|-------|
//! | **ERROR** | Operation failed; session/feature impacted; `Err` return or abort | Normal validation noise |
//! | **WARN** | Handled degradation: retry, fallback, timeout, partial success, recovered anomaly | Steady-state success |
//! | **INFO** | Rare durable facts: lifecycle, one-line session/update outcome, startup config | Per-frame, per-packet, inner-loop OK |
//! | **DEBUG** | State transitions, branches, sizes/counts (sampled), handshake steps, codec choice | Secrets; unbounded hot-path spam |
//! | **TRACE** | Max verbosity when `RMM_DEBUG` forces `trace` | Same as DEBUG unless extra detail is needed |
//!
//! Hot loops (encode, input): use sampled DEBUG or `tracing::enabled!` guards.

use serde::{Deserialize, Serialize};

/// Shared opt-in setting for the endpoint used to discover a public UDP address.
///
/// Talos does not ship a third-party STUN default. An unset or whitespace-only value disables
/// STUN, allowing the interactive transports to use their negotiated relay fallback without an
/// undeclared external network request.
pub const RMM_STUN_SERVER_ENV: &str = "RMM_STUN_SERVER";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StunServerConfigError {
    NonUnicode,
    TooLong,
    MissingPort,
    InvalidHost,
    InvalidPort,
}

impl std::fmt::Display for StunServerConfigError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let message = match self {
            Self::NonUnicode => "RMM_STUN_SERVER must be valid Unicode",
            Self::TooLong => "RMM_STUN_SERVER exceeds 260 characters",
            Self::MissingPort => {
                "RMM_STUN_SERVER must use the hostname-or-IPv4:port form without a URL scheme"
            }
            Self::InvalidHost => {
                "RMM_STUN_SERVER host must be a valid DNS hostname, localhost, or IPv4 address"
            }
            Self::InvalidPort => "RMM_STUN_SERVER port must be an integer from 1 through 65535",
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for StunServerConfigError {}

/// Validates the shared STUN endpoint contract without resolving DNS or opening a socket.
///
/// The accepted representation is deliberately narrower than a URL: a DNS hostname or IPv4
/// address followed by a non-zero port. This keeps the worker and viewer behavior identical and
/// rejects credentials, schemes, paths, query strings, fragments, and ambiguous IPv6 literals.
pub fn parse_stun_server(value: Option<&str>) -> Result<Option<String>, StunServerConfigError> {
    let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    if value.len() > 260 {
        return Err(StunServerConfigError::TooLong);
    }

    let Some((host, port)) = value.rsplit_once(':') else {
        return Err(StunServerConfigError::MissingPort);
    };
    if host.is_empty() || port.is_empty() || host.contains(':') {
        return Err(StunServerConfigError::MissingPort);
    }
    let port = port
        .parse::<u16>()
        .map_err(|_| StunServerConfigError::InvalidPort)?;
    if port == 0 {
        return Err(StunServerConfigError::InvalidPort);
    }

    let valid_ipv4 = host.parse::<std::net::Ipv4Addr>().is_ok();
    let valid_hostname = host.len() <= 253
        && host.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
                && label
                    .as_bytes()
                    .first()
                    .is_some_and(u8::is_ascii_alphanumeric)
                && label
                    .as_bytes()
                    .last()
                    .is_some_and(u8::is_ascii_alphanumeric)
        });
    if !valid_ipv4 && !valid_hostname {
        return Err(StunServerConfigError::InvalidHost);
    }

    Ok(Some(format!("{host}:{port}")))
}

/// Reads and validates [`RMM_STUN_SERVER_ENV`]. An absent value intentionally returns `None`.
pub fn configured_stun_server() -> Result<Option<String>, StunServerConfigError> {
    match std::env::var(RMM_STUN_SERVER_ENV) {
        Ok(value) => parse_stun_server(Some(&value)),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(std::env::VarError::NotUnicode(_)) => Err(StunServerConfigError::NonUnicode),
    }
}

/// Returns `true` only when `RMM_DEBUG` is set to an explicit affirmative (`1`, `true`, `yes`,
/// `y`, `on`), case-insensitive after trim.
///
/// Any other value — including empty, `0`, `no`, `off`, `debug`, or arbitrary strings — is treated
/// as off so shipped binaries do not pick up accidental trace verbosity from partial env blocks
/// or legacy `RMM_DEBUG=warn`-style mistakes.
///
/// Shared by the agent, helper, and viewer for verbose tracing output and debug-only behavior
/// (for example the viewer allocating a Windows console).
///
/// When `true`, [`rmm_tracing_filter_directive`] returns `trace` so all tracing levels are emitted.
pub fn rmm_debug_enabled() -> bool {
    match std::env::var("RMM_DEBUG") {
        Ok(value) => {
            let normalized = value.trim().to_ascii_lowercase();
            matches!(normalized.as_str(), "1" | "true" | "yes" | "y" | "on")
        }
        Err(_) => false,
    }
}

/// Tracing env-filter directive string for `tracing_subscriber::EnvFilter::new` in Talos shipped Windows binaries
/// (agent, helper, viewer, updaters).
///
/// Precedence:
/// 1. **`RMM_DEBUG`** is one of `1` / `true` / `yes` / `y` / `on` (case-insensitive) → `trace`.
/// 2. **`RMM_LOGLEVEL`** (case-insensitive, trimmed): `error`, `warn`/`warning`, `info`, `debug`, or `trace`.
///    Unknown values fall back to `warn`.
/// 3. Default → `warn` (warnings and errors only; suppresses `info!` / `debug!` / `trace!` from crates using this filter).
///
/// Does not read `RUST_LOG`; callers typically clear that variable so this policy applies consistently.
pub fn rmm_tracing_filter_directive() -> String {
    if rmm_debug_enabled() {
        return "trace".to_string();
    }
    if let Ok(raw) = std::env::var("RMM_LOGLEVEL") {
        let s = raw.trim().to_ascii_lowercase();
        return match s.as_str() {
            "" => "warn".to_string(),
            "error" => "error".to_string(),
            "warn" | "warning" => "warn".to_string(),
            "info" => "info".to_string(),
            "debug" => "debug".to_string(),
            "trace" => "trace".to_string(),
            _ => "warn".to_string(),
        };
    }
    "warn".to_string()
}

#[cfg(feature = "relay-transport")]
pub mod relay_transport;

// -----------------------------------------------------------------------------
// Relay heartbeat (viewer → agent)
// -----------------------------------------------------------------------------

/// Payload sent by the viewer every 15 seconds. Agent treats 3 missed heartbeats as viewer disconnect.
pub const HEARTBEAT_PAYLOAD: &[u8] = b"heartbeat";

/// Helper pipe handshake magic shared by the agent and helper processes.
pub const HELPER_PIPE_HANDSHAKE_MAGIC: [u8; 4] = *b"RMMH";
/// Agent/helper pipe protocol version. Bump on any incompatible handshake change.
pub const HELPER_PIPE_PROTOCOL_VERSION: u16 = 1;
/// Maximum auth token length accepted on agent/helper pipe handshakes.
pub const HELPER_PIPE_MAX_AUTH_TOKEN_LEN: usize = 128;

// -----------------------------------------------------------------------------
// Binary control protocol (viewer → agent)
// -----------------------------------------------------------------------------

/// Maximum payload length for a single control message (u16 length prefix).
pub const CONTROL_MAX_PAYLOAD_LEN: usize = u16::MAX as usize;

/// Control message type: mouse move (x,y).
pub const CONTROL_TYPE_MOUSE_MOVE: u8 = 0x01;
/// Control message type: mouse button.
pub const CONTROL_TYPE_MOUSE_BUTTON: u8 = 0x02;
/// Control message type: mouse wheel.
pub const CONTROL_TYPE_MOUSE_WHEEL: u8 = 0x03;
/// Control message type: key down.
pub const CONTROL_TYPE_KEY_DOWN: u8 = 0x04;
/// Control message type: key up.
pub const CONTROL_TYPE_KEY_UP: u8 = 0x05;
/// Control message type: clipboard paste.
pub const CONTROL_TYPE_CLIPBOARD: u8 = 0x06;
/// Control message type: typed input (unicode per character).
pub const CONTROL_TYPE_TYPED_INPUT: u8 = 0x07;
/// Control message type: switch capture/input target to a specific Windows session id.
/// Payload: target session id u32 BE (4 bytes).
pub const CONTROL_TYPE_SESSION_SWITCH: u8 = 0x08;
/// Control message type: log off a specific Windows session id.
/// Payload: target session id u32 BE (4 bytes).
pub const CONTROL_TYPE_SESSION_LOGOFF: u8 = 0x09;
/// Control message type: remote registry request (viewer → agent).
/// Payload: UTF-8 JSON encoded `RegistryRequest`.
pub const CONTROL_TYPE_REGISTRY_REQUEST: u8 = 0x0A;
/// Control message type: agent → helper, request capture loop to stop (e.g. session closed).
/// Payload: empty.
pub const CONTROL_TYPE_STOP_CAPTURE: u8 = 0x0B;
/// Control message type: viewer → agent RTT probe.
/// Payload: viewer send time in unix milliseconds (u64 BE).
pub const CONTROL_TYPE_CONNECTION_PING: u8 = 0x0C;
/// Control message type reserved for future transport telemetry pong frames.
pub const CONTROL_TYPE_CONNECTION_PONG: u8 = 0x0D;
/// Control message type: viewer → agent secure-attention request (Ctrl+Alt+Del equivalent).
/// Payload: empty.
pub const CONTROL_TYPE_SECURE_ATTENTION: u8 = 0x0E;
/// Control message type: viewer → helper (via agent forward), switch DXGI capture output index.
/// Payload: `u32` capture output index BE (4 bytes), matching DXGI enumeration order on the agent.
pub const CONTROL_TYPE_CAPTURE_OUTPUT_SWITCH: u8 = 0x0F;
/// Control message type: viewer → helper (via agent forward), request a new stream bitrate.
/// Payload: target bitrate in kilobits per second as `u32` BE (4 bytes).
pub const CONTROL_TYPE_STREAM_BITRATE: u8 = 0x10;
/// Control message type: mouse double-click.
/// Payload: button u8, normalized x u32 BE, normalized y u32 BE.
pub const CONTROL_TYPE_MOUSE_DOUBLE_CLICK: u8 = 0x11;

/// Modifier bits for keyboard messages.
pub const CONTROL_MOD_CTRL: u8 = 0x01;
pub const CONTROL_MOD_SHIFT: u8 = 0x02;
pub const CONTROL_MOD_ALT: u8 = 0x04;
pub const CONTROL_MOD_WIN: u8 = 0x08;

/// Fixed payload sizes (excluding the 2B length and 1B type prefix).
pub const CONTROL_PAYLOAD_MOUSE_MOVE_LEN: usize = 8;
pub const CONTROL_PAYLOAD_MOUSE_BUTTON_LEN: usize = 10;
pub const CONTROL_PAYLOAD_MOUSE_DOUBLE_CLICK_LEN: usize = 9;
pub const CONTROL_PAYLOAD_MOUSE_WHEEL_LEN: usize = 10;
pub const CONTROL_PAYLOAD_KEY_LEN: usize = 5;
/// Fixed payload size for session switch/logoff control messages.
pub const CONTROL_PAYLOAD_SESSION_ID_LEN: usize = 4;
/// Fixed payload size for capture output switch (DXGI output index).
pub const CONTROL_PAYLOAD_CAPTURE_OUTPUT_INDEX_LEN: usize = 4;
/// Fixed payload size for live stream bitrate control.
pub const CONTROL_PAYLOAD_STREAM_BITRATE_LEN: usize = 4;
/// Fixed payload size for transport telemetry ping/pong timestamps.
pub const CONTROL_PAYLOAD_TIMESTAMP_LEN: usize = 8;

#[derive(Debug, Clone, Copy)]
pub struct ControlFrame<'a> {
    pub message_type: u8,
    pub payload: &'a [u8],
}

#[derive(Debug, Clone)]
pub enum ControlParseError {
    TooShort,
    LengthMismatch,
    LengthOverflow,
}

impl std::fmt::Display for ControlParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ControlParseError::TooShort => write!(f, "control frame too short"),
            ControlParseError::LengthMismatch => write!(f, "control frame length mismatch"),
            ControlParseError::LengthOverflow => write!(f, "control frame length overflow"),
        }
    }
}

impl std::error::Error for ControlParseError {}

#[derive(Debug, Clone)]
pub enum ControlEncodeError {
    PayloadTooLarge,
}

impl std::fmt::Display for ControlEncodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ControlEncodeError::PayloadTooLarge => write!(f, "control payload too large"),
        }
    }
}

impl std::error::Error for ControlEncodeError {}

/// Parse a control frame (2B length + 1B type + payload).
pub fn parse_control_frame(frame: &[u8]) -> Result<ControlFrame<'_>, ControlParseError> {
    if frame.len() < 3 {
        return Err(ControlParseError::TooShort);
    }
    let length = u16::from_be_bytes([frame[0], frame[1]]) as usize;
    if length > CONTROL_MAX_PAYLOAD_LEN {
        return Err(ControlParseError::LengthOverflow);
    }
    if frame.len() != 3 + length {
        return Err(ControlParseError::LengthMismatch);
    }
    Ok(ControlFrame {
        message_type: frame[2],
        payload: &frame[3..],
    })
}

/// Build a control frame (2B length + 1B type + payload).
pub fn build_control_frame(
    message_type: u8,
    payload: &[u8],
) -> Result<Vec<u8>, ControlEncodeError> {
    if payload.len() > CONTROL_MAX_PAYLOAD_LEN {
        return Err(ControlEncodeError::PayloadTooLarge);
    }
    let mut out = Vec::with_capacity(3 + payload.len());
    out.extend_from_slice(&(payload.len() as u16).to_be_bytes());
    out.push(message_type);
    out.extend_from_slice(payload);
    Ok(out)
}

// -----------------------------------------------------------------------------
// Network types
// -----------------------------------------------------------------------------

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
pub struct ReflexAddress {
    pub ip: String,
    pub port: u16,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
pub struct LocalAddr {
    pub ip: String,
    pub prefix: u8,
}

/// Transport/session mode used for `tunnel_prepare` and relay setup.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum SessionTransportMode {
    #[default]
    RemoteDesktop,
    HeadlessRemoteDesktop,
    FileTransfer,
    RemoteRegistry,
    Shell,
    Chat,
}

/// Shared feature kind used across interactive RMM sessions.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SessionFeatureKind {
    RemoteDesktop,
    SystemShell,
    FileTransfer,
    RemoteRegistry,
    Chat,
}

/// Managed endpoint platform reported by the agent.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum AgentPlatform {
    Windows,
    Linux,
    Macos,
    #[default]
    Unknown,
}

/// Feature availability for a connected agent.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct AgentFeatureCapabilities {
    pub remote_desktop: bool,
    pub system_shell: bool,
    pub file_transfer: bool,
    pub remote_registry: bool,
    pub chat: bool,
    pub system_info: bool,
}

impl AgentFeatureCapabilities {
    pub fn windows() -> Self {
        Self {
            remote_desktop: true,
            system_shell: true,
            file_transfer: true,
            remote_registry: true,
            chat: true,
            system_info: true,
        }
    }

    pub fn linux() -> Self {
        Self {
            remote_desktop: false,
            system_shell: true,
            file_transfer: true,
            remote_registry: false,
            chat: false,
            system_info: true,
        }
    }

    pub fn macos() -> Self {
        Self {
            remote_desktop: true,
            system_shell: true,
            file_transfer: true,
            remote_registry: false,
            chat: true,
            system_info: true,
        }
    }

    pub fn unsupported() -> Self {
        Self {
            remote_desktop: false,
            system_shell: false,
            file_transfer: false,
            remote_registry: false,
            chat: false,
            system_info: true,
        }
    }

    pub fn for_platform(platform: AgentPlatform) -> Self {
        match platform {
            AgentPlatform::Windows => Self::windows(),
            AgentPlatform::Linux => Self::linux(),
            AgentPlatform::Macos => Self::macos(),
            AgentPlatform::Unknown => Self::unsupported(),
        }
    }
}

impl Default for AgentFeatureCapabilities {
    fn default() -> Self {
        // Backward-compatible default for older agents that predate explicit
        // feature advertisement. Current Linux/macOS agents send explicit
        // platform-limited capabilities.
        Self::windows()
    }
}

/// Shared lifecycle state for interactive RMM feature sessions.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SessionLifecycleState {
    Connecting,
    Negotiating,
    Active,
    Degraded,
    Reconnecting,
    Closing,
    Closed,
    Failed,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OperationErrorCode {
    Timeout,
    TransportLost,
    PeerClosed,
    Backpressure,
    PermissionDenied,
    NoInteractiveUser,
    PayloadTooLarge,
    Conflict,
    Cancelled,
    StaleSession,
    InvalidRequest,
    InvalidPath,
    InvalidType,
    NotFound,
    Unsupported,
    Internal,
    #[serde(other)]
    Unknown,
}

// -----------------------------------------------------------------------------
// Session kind and capability structs (Option A)
// -----------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Clone)]
pub enum SessionKind {
    RemoteDesktop(RemoteDesktopCapabilities),
    SystemShell(SystemShellCapabilities),
    RemoteRegistry(RegistryCapabilities),
    FileTransfer(FileTransferCapabilities),
    Chat(ChatCapabilities),
}

pub const REMOTE_DESKTOP_PROFILE_MODERN_GPU: &str = "modern_gpu";
pub const REMOTE_DESKTOP_PROFILE_MODERN_CPU: &str = "modern_cpu";
pub const REMOTE_DESKTOP_PROFILE_LEGACY: &str = "legacy";
pub const REMOTE_DESKTOP_PROFILE_EXPERIMENTAL: &str = "experimental";
pub const REMOTE_DESKTOP_PROFILE_SCREENSHOT_ONLY: &str = "screenshot_only";
pub const REMOTE_DESKTOP_PROTOCOL_LEGACY_IVF: &str = "legacy_ivf";
pub const REMOTE_DESKTOP_PROTOCOL_MODERN_DISPLAY_DELTA: &str = "modern_display_delta";
pub const REMOTE_DESKTOP_PROTOCOL_EXPERIMENTAL_DISPLAY_DELTA: &str = "experimental_display_delta";
pub const REMOTE_DESKTOP_PROTOCOL_SCREENSHOT_ONLY: &str = "screenshot_only";
pub const REMOTE_DESKTOP_CODEC_BGRA_ATLAS_COMMANDS: &str = "bgra_atlas_commands";
pub const REMOTE_DESKTOP_CODEC_H264: &str = "h264";
pub const REMOTE_DESKTOP_CODEC_VP8: &str = "vp8";
pub const REMOTE_DESKTOP_CODEC_SCREENSHOT_BGRA: &str = "screenshot_bgra";
pub const REMOTE_DESKTOP_COMPRESSION_GPU_TILE_COMMANDS: &str = "gpu_tile_commands";
pub const REMOTE_DESKTOP_COMPRESSION_ANNEX_B: &str = "annex_b";
pub const REMOTE_DESKTOP_COMPRESSION_IVF: &str = "ivf";
pub const REMOTE_DESKTOP_COMPRESSION_NONE: &str = "none";

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct RemoteDesktopDisplayProfile {
    pub id: String,
    pub protocol: String,
    pub codec: String,
    pub compression: String,
    pub priority: u8,
}

impl RemoteDesktopDisplayProfile {
    pub fn experimental() -> Self {
        Self {
            id: REMOTE_DESKTOP_PROFILE_EXPERIMENTAL.to_string(),
            protocol: REMOTE_DESKTOP_PROTOCOL_EXPERIMENTAL_DISPLAY_DELTA.to_string(),
            codec: REMOTE_DESKTOP_CODEC_BGRA_ATLAS_COMMANDS.to_string(),
            compression: REMOTE_DESKTOP_COMPRESSION_GPU_TILE_COMMANDS.to_string(),
            priority: 0,
        }
    }

    pub fn modern_gpu() -> Self {
        Self {
            id: REMOTE_DESKTOP_PROFILE_MODERN_GPU.to_string(),
            protocol: REMOTE_DESKTOP_PROTOCOL_MODERN_DISPLAY_DELTA.to_string(),
            codec: REMOTE_DESKTOP_CODEC_H264.to_string(),
            compression: REMOTE_DESKTOP_COMPRESSION_ANNEX_B.to_string(),
            priority: 0,
        }
    }

    pub fn legacy() -> Self {
        Self {
            id: REMOTE_DESKTOP_PROFILE_LEGACY.to_string(),
            protocol: REMOTE_DESKTOP_PROTOCOL_LEGACY_IVF.to_string(),
            codec: REMOTE_DESKTOP_CODEC_VP8.to_string(),
            compression: REMOTE_DESKTOP_COMPRESSION_IVF.to_string(),
            priority: 2,
        }
    }

    pub fn screenshot_only() -> Self {
        Self {
            id: REMOTE_DESKTOP_PROFILE_SCREENSHOT_ONLY.to_string(),
            protocol: REMOTE_DESKTOP_PROTOCOL_SCREENSHOT_ONLY.to_string(),
            codec: REMOTE_DESKTOP_CODEC_SCREENSHOT_BGRA.to_string(),
            compression: REMOTE_DESKTOP_COMPRESSION_NONE.to_string(),
            priority: 10,
        }
    }
}

/// Remote desktop session capabilities (codecs, encoding, transports, display profiles).
#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct RemoteDesktopCapabilities {
    pub codecs: Vec<String>,
    pub encoding: String,
    pub transports: Vec<String>,
    #[serde(default)]
    pub platform: AgentPlatform,
    #[serde(default)]
    pub features: AgentFeatureCapabilities,
    #[serde(default)]
    pub display_profiles: Vec<RemoteDesktopDisplayProfile>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected_display_profile: Option<String>,
}

/// Placeholder for future system shell session capabilities.
#[derive(Serialize, Deserialize, Clone, Default)]
pub struct SystemShellCapabilities {}

/// Remote registry session capabilities.
#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct RegistryCapabilities {
    pub transports: Vec<String>,
}

impl Default for RegistryCapabilities {
    fn default() -> Self {
        Self {
            transports: vec!["quic".to_string(), "relay".to_string()],
        }
    }
}

// -----------------------------------------------------------------------------
// Remote registry request/response types (viewer ↔ agent)
// -----------------------------------------------------------------------------

/// Remote registry hive selection.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum RegistryHive {
    HKLM,
    HKCU,
    HKCR,
    HKU,
    HKCC,
}

/// Registry value data. Binary values are base64-encoded to keep JSON compact.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum RegistryValueData {
    Sz {
        data: String,
    },
    Dword {
        data: u32,
    },
    /// Stored as a decimal string because JS/JSON cannot represent full u64 precisely.
    Qword {
        data: String,
    },
    MultiSz {
        data: Vec<String>,
    },
    Binary {
        data_b64: String,
    },
    /// Used for value types outside of the structured variants above.
    /// `raw_type` is the Windows REG_* numeric constant.
    Unknown {
        raw_type: u32,
        data_b64: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RegistryValueEntry {
    /// Value name. Empty string represents the default value.
    pub name: String,
    /// Windows type name (e.g. \"REG_SZ\", \"REG_DWORD\").
    pub value_type: String,
    /// Raw Windows REG_* numeric constant.
    pub raw_type: u32,
    pub data: RegistryValueData,
}

/// Registry request sent viewer → agent (over the control channel).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum RegistryRequest {
    ListKeys {
        request_id: String,
        session_id: String,
        hive: RegistryHive,
        /// Key path within the hive. Empty string refers to the hive root.
        path: String,
        #[serde(default)]
        offset: u32,
        #[serde(default)]
        limit: u32,
    },
    ListValues {
        request_id: String,
        session_id: String,
        hive: RegistryHive,
        path: String,
        #[serde(default)]
        offset: u32,
        #[serde(default)]
        limit: u32,
    },
    GetValue {
        request_id: String,
        session_id: String,
        hive: RegistryHive,
        path: String,
        name: String,
    },
    SetValue {
        request_id: String,
        session_id: String,
        hive: RegistryHive,
        path: String,
        name: String,
        data: RegistryValueData,
    },
    CreateKey {
        request_id: String,
        session_id: String,
        hive: RegistryHive,
        /// Full key path to create.
        path: String,
    },
    DeleteKey {
        request_id: String,
        session_id: String,
        hive: RegistryHive,
        /// Full key path to delete.
        path: String,
        /// When true, delete recursively (RegDeleteTree semantics).
        recursive: bool,
    },
    DeleteValue {
        request_id: String,
        session_id: String,
        hive: RegistryHive,
        path: String,
        name: String,
    },
    Cancel {
        request_id: String,
        session_id: String,
        target_request_id: String,
    },
}

pub const REGISTRY_META_MESSAGE_TYPE: &str = "registry_response";

/// Response envelope sent agent → viewer (over the metadata channel).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RegistryResponseEnvelope {
    #[serde(rename = "type")]
    pub message_type: String,
    pub request_id: String,
    pub session_id: String,
    pub response: RegistryResponse,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RegistryResponse {
    ListKeys {
        keys: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        next_offset: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        total_count: Option<u32>,
    },
    ListValues {
        values: Vec<RegistryValueEntry>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        next_offset: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        total_count: Option<u32>,
    },
    GetValue {
        value: Option<RegistryValueEntry>,
    },
    Ok {},
    Error {
        code: OperationErrorCode,
        message: String,
    },
}

pub type RegistryErrorCode = OperationErrorCode;

/// Placeholder for future file transfer session capabilities.
#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct FileTransferCapabilities {
    pub transports: Vec<String>,
    pub zip_threshold_files: u32,
    pub zip_threshold_bytes: u64,
    pub max_chunk_bytes: u32,
}

impl Default for FileTransferCapabilities {
    fn default() -> Self {
        Self {
            transports: vec!["quic".to_string(), "relay".to_string()],
            zip_threshold_files: FILE_TRANSFER_DEFAULT_ZIP_THRESHOLD_FILES,
            zip_threshold_bytes: FILE_TRANSFER_DEFAULT_ZIP_THRESHOLD_BYTES,
            max_chunk_bytes: FILE_TRANSFER_DEFAULT_CHUNK_BYTES,
        }
    }
}

/// Session chat capabilities (transport paths).
#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ChatCapabilities {
    pub transports: Vec<String>,
}

impl Default for ChatCapabilities {
    fn default() -> Self {
        Self {
            transports: vec!["quic".to_string(), "relay".to_string()],
        }
    }
}

/// Alias for backward compatibility.
pub type SessionCapabilities = RemoteDesktopCapabilities;

// -----------------------------------------------------------------------------
// Wire protocol types (agent–server WebSocket)
// -----------------------------------------------------------------------------

#[derive(Serialize)]
pub struct OutgoingEnvelope<T>
where
    T: Serialize,
{
    #[serde(rename = "type")]
    pub message_type: &'static str,
    pub data: T,
}

#[derive(Debug, Clone, Deserialize)]
pub struct IncomingEnvelope {
    #[serde(rename = "type")]
    pub message_type: String,
    pub data: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AgentHello {
    pub agent_id: String,
    pub hostname: String,
    pub os: String,
    pub ip: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_addrs: Option<Vec<LocalAddr>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default)]
    pub is_admin: bool,
    #[serde(default)]
    pub platform: AgentPlatform,
    #[serde(default)]
    pub features: AgentFeatureCapabilities,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FullSnapshotUpdate {
    pub agent_id: String,
    pub collected_at: String,
    pub snapshot: serde_json::Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot_request_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TelemetryEventsUpdate {
    pub agent_id: String,
    #[serde(default)]
    pub events: Vec<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct RequestFullSnapshotPayload {
    #[serde(
        rename = "snapshotRequestId",
        alias = "snapshot_request_id",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub snapshot_request_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ShellCommandPayload {
    pub request_id: String,
    pub command: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ShellOutputPayload {
    pub request_id: String,
    pub output: String,
    pub exit_code: Option<i32>,
}

#[derive(Serialize, Deserialize)]
pub struct SessionCapabilitiesRequest {
    pub request_id: String,
}

#[derive(Serialize, Deserialize)]
pub struct SessionCapabilitiesResponse {
    pub request_id: String,
    pub capabilities: RemoteDesktopCapabilities,
}

// -----------------------------------------------------------------------------
// Remote desktop QUIC/relay payloads
// -----------------------------------------------------------------------------

#[derive(Serialize, Deserialize)]
pub struct TunnelPreparePayload {
    pub session_id: String,
    pub psk_cert_pem: String,
    pub psk_key_pem: String,
    pub relay_url: Option<String>,
    pub e2e_key: Option<String>,
    #[serde(default)]
    pub mode: SessionTransportMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected_display_profile: Option<String>,
    #[serde(default)]
    pub hide_cursor: bool,
    /// Viewer bearer token for first QUIC/relay chat handshake (`CHAT_MSG_AUTH`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub viewer_session_token: Option<String>,
    /// Remote-desktop session id this chat targets for resolving interactive Windows session id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_desktop_session_id: Option<String>,
}

#[derive(Serialize, Deserialize)]
pub struct QuicReflexPayload {
    pub session_id: String,
    pub reflex: ReflexAddress,
}

#[derive(Serialize, Deserialize)]
pub struct RelayPreparePayload {
    pub session_id: String,
    pub relay_url: String,
    pub e2e_key: String,
    #[serde(default)]
    pub mode: SessionTransportMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected_display_profile: Option<String>,
    #[serde(default)]
    pub hide_cursor: bool,
}

#[derive(Serialize, Deserialize)]
pub struct PunchStartPayload {
    pub session_id: String,
    pub peer_reflex: ReflexAddress,
}

// -----------------------------------------------------------------------------
// Remote desktop unavailable (agent → server)
// -----------------------------------------------------------------------------

/// Sent by the agent when desktop capture cannot start. The server completes
/// the pending connect flow with an
/// error so the viewer never receives a session URL.
#[derive(Serialize, Deserialize)]
pub struct RemoteDesktopUnavailablePayload {
    pub session_id: String,
    /// Reason code like `"no_display"`.
    pub reason: String,
    /// Optional human-readable detail.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

// -----------------------------------------------------------------------------
// Interactive shell TCP stream protocol (viewer ↔ agent)
// -----------------------------------------------------------------------------

/// Shell TCP frame: `[1B type][2B payload length BE][payload]`.
/// Maximum payload length matches control frame limit.
pub const SHELL_MAX_PAYLOAD_LEN: usize = u16::MAX as usize;

/// Shell message: authenticate (viewer → agent). Payload: UTF-8 token string.
pub const SHELL_MSG_AUTH: u8 = 0x01;
/// Shell message: terminal input bytes (viewer → agent).
pub const SHELL_MSG_INPUT: u8 = 0x02;
/// Shell message: terminal output bytes (agent → viewer).
pub const SHELL_MSG_OUTPUT: u8 = 0x03;
/// Shell message: resize terminal (viewer → agent). Payload: cols u16 BE + rows u16 BE.
pub const SHELL_MSG_RESIZE: u8 = 0x04;
/// Shell message: process exited (agent → viewer). Payload: exit_code u32 BE.
pub const SHELL_MSG_EXIT: u8 = 0x05;
/// Shell message: error (agent → viewer). Payload: UTF-8 error string.
pub const SHELL_MSG_ERROR: u8 = 0x06;

/// Fixed payload size for resize: cols (2B) + rows (2B).
pub const SHELL_RESIZE_PAYLOAD_LEN: usize = 4;
/// Fixed payload size for exit: exit_code (4B).
pub const SHELL_EXIT_PAYLOAD_LEN: usize = 4;

/// Parsed shell frame.
#[derive(Debug, Clone)]
pub struct ShellFrame {
    pub message_type: u8,
    pub payload: Vec<u8>,
}

#[derive(Debug, Clone)]
pub enum ShellFrameError {
    TooShort,
    LengthOverflow,
    Io(String),
}

impl std::fmt::Display for ShellFrameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ShellFrameError::TooShort => write!(f, "shell frame too short"),
            ShellFrameError::LengthOverflow => write!(f, "shell frame payload too large"),
            ShellFrameError::Io(e) => write!(f, "shell frame io: {e}"),
        }
    }
}

impl std::error::Error for ShellFrameError {}

/// Build a shell frame: `[1B type][2B length BE][payload]`.
pub fn build_shell_frame(message_type: u8, payload: &[u8]) -> Result<Vec<u8>, ShellFrameError> {
    if payload.len() > SHELL_MAX_PAYLOAD_LEN {
        return Err(ShellFrameError::LengthOverflow);
    }
    let mut out = Vec::with_capacity(3 + payload.len());
    out.push(message_type);
    out.extend_from_slice(&(payload.len() as u16).to_be_bytes());
    out.extend_from_slice(payload);
    Ok(out)
}

/// Build a shell resize payload: cols u16 BE + rows u16 BE.
pub fn build_shell_resize_payload(cols: u16, rows: u16) -> [u8; SHELL_RESIZE_PAYLOAD_LEN] {
    let mut buf = [0u8; SHELL_RESIZE_PAYLOAD_LEN];
    buf[0..2].copy_from_slice(&cols.to_be_bytes());
    buf[2..4].copy_from_slice(&rows.to_be_bytes());
    buf
}

/// Parse a shell resize payload into (cols, rows).
pub fn parse_shell_resize_payload(payload: &[u8]) -> Option<(u16, u16)> {
    if payload.len() < SHELL_RESIZE_PAYLOAD_LEN {
        return None;
    }
    let cols = u16::from_be_bytes([payload[0], payload[1]]);
    let rows = u16::from_be_bytes([payload[2], payload[3]]);
    Some((cols, rows))
}

/// Build a shell exit payload: exit_code u32 BE.
pub fn build_shell_exit_payload(exit_code: u32) -> [u8; SHELL_EXIT_PAYLOAD_LEN] {
    exit_code.to_be_bytes()
}

/// Parse a shell exit payload into exit_code.
pub fn parse_shell_exit_payload(payload: &[u8]) -> Option<u32> {
    if payload.len() < SHELL_EXIT_PAYLOAD_LEN {
        return None;
    }
    Some(u32::from_be_bytes([
        payload[0], payload[1], payload[2], payload[3],
    ]))
}

// -----------------------------------------------------------------------------
// Interactive shell session wire types (agent ↔ server WebSocket JSON)
// -----------------------------------------------------------------------------

/// How the shell process should run on the agent.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ShellRunAs {
    User,
    System,
}

/// Server → agent: start an interactive shell session.
///
/// Note: `rename_all = "camelCase"` matches the server's serialisation so the
/// agent can deserialise the payload without any mapping layer.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShellStartPayload {
    pub session_id: String,
    pub token: String,
    pub run_as: ShellRunAs,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_session_id: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relay_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub e2e_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub psk_cert_pem: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub psk_key_pem: Option<String>,
}

/// Agent -> server: managed Linux shell sudo credential generated locally.
#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct LinuxShellCredentialPayload {
    pub agent_id: String,
    pub username: String,
    pub password: String,
    pub credential_id: String,
    pub version: i32,
    pub generated_at: String,
}

/// Server -> agent: API accepted the managed Linux shell sudo credential.
#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct LinuxShellCredentialStoredPayload {
    pub credential_id: String,
    pub stored_at: String,
}

// -----------------------------------------------------------------------------
// macOS managed software update account
// -----------------------------------------------------------------------------

/// Fixed Unix-domain socket used by the root worker and the GUI permissions helper
/// for local macOS software update account enrollment.
pub const MACOS_UPDATE_ACCOUNT_SOCKET_PATH: &str = "/var/run/talos/macos-update-account.sock";

/// Non-secret status for the local macOS account used to authenticate
/// Apple Silicon `softwareupdate` installs.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct MacosUpdateAccountStatus {
    pub schema_version: u32,
    pub required: bool,
    pub status: String,
    pub username: String,
    pub is_apple_silicon: bool,
    pub account_present: bool,
    pub is_admin: bool,
    pub is_volume_owner: bool,
    pub secure_token_enabled: bool,
    pub credential_available: bool,
    pub credential_version: Option<i32>,
    pub generated_uid: Option<String>,
    pub expected_generated_uid: Option<String>,
    pub discovered_volume_owners: Vec<MacosVolumeOwnerUser>,
    pub failure_code: Option<String>,
    pub failure_message: Option<String>,
    pub checked_at: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct MacosVolumeOwnerUser {
    pub username: Option<String>,
    pub full_name: Option<String>,
    pub generated_uid: Option<String>,
    pub volume_owner: bool,
}

/// Agent -> server: latest non-secret macOS update-account status.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct MacosUpdateAccountStatusPayload {
    pub agent_id: String,
    pub status: MacosUpdateAccountStatus,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct MacosUpdateEnrollmentAccount {
    pub username: String,
    pub password: String,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum MacosUpdateAccountIpcRequest {
    GetStatus,
    BeginInteractiveEnrollment,
    CompleteInteractiveEnrollment {
        session_id: String,
        sysadminctl_succeeded: bool,
        sysadminctl_output: String,
    },
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct MacosUpdateAccountIpcResponse {
    pub ok: bool,
    pub status: Option<MacosUpdateAccountStatus>,
    pub session_id: Option<String>,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enrollment_account: Option<MacosUpdateEnrollmentAccount>,
}

/// Agent → server: shell session ready, viewer can connect.
#[derive(Serialize, Deserialize)]
pub struct ShellOfferPayload {
    pub session_id: String,
    pub stream_port: u16,
    pub host: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub local_addrs: Vec<LocalAddr>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reflex: Option<ReflexAddress>,
}

/// HTTP response for shell session capabilities (viewer fetches before connecting).
#[derive(Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct ShellSessionCapabilitiesHttpResponse {
    pub transports: Vec<String>,
    #[serde(default)]
    pub platform: AgentPlatform,
    #[serde(default)]
    pub features: AgentFeatureCapabilities,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_reflex: Option<ReflexAddress>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_host: Option<String>,
    #[serde(default)]
    pub agent_local_addrs: Vec<LocalAddr>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub psk_cert_pem: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relay_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub e2e_key: Option<String>,
}

// -----------------------------------------------------------------------------
// RMM chat framed stream (viewer ↔ agent over QUIC / relay E2E, agent ↔ sidecar pipe)
// -----------------------------------------------------------------------------

/// Chat TCP-like frame: `[1B type][2B payload length BE][payload]`.
pub const CHAT_MAX_PAYLOAD_LEN: usize = u16::MAX as usize;

/// Viewer/agent authenticate stream with viewer bearer token (UTF-8), matching tunnel_prepare viewer_session_token.
pub const CHAT_MSG_AUTH: u8 = 0x01;
/// Chat payload JSON encoded [`ChatWirePayload`].
pub const CHAT_MSG_TEXT: u8 = 0x02;
/// Delivery/read acknowledgement JSON [`ChatAckPayload`].
pub const CHAT_MSG_ACK: u8 = 0x03;
/// Graceful close; payload optional UTF-8 reason.
pub const CHAT_MSG_CLOSE: u8 = 0x04;
/// UTF-8 error text or JSON [`ChatWireErrorPayload`].
pub const CHAT_MSG_ERROR: u8 = 0x05;
/// Worker-local sidecar control payload JSON encoded [`WorkerChatControlPayload`].
pub const CHAT_MSG_CONTROL: u8 = 0x06;

/// Pipe handshake between `talos_worker` (service) and `talos_worker_chat` (user session).
pub const CHAT_PIPE_HANDSHAKE_MAGIC: [u8; 4] = *b"RMMC";
pub const CHAT_PIPE_PROTOCOL_VERSION: u16 = 1;
pub const CHAT_PIPE_MAX_AUTH_TOKEN_LEN: usize = 512;

#[derive(Debug, Clone)]
pub struct ChatFrame {
    pub message_type: u8,
    pub payload: Vec<u8>,
}

#[derive(Debug, Clone)]
pub enum ChatFrameError {
    TooShort,
    LengthMismatch,
    LengthOverflow,
    Io(String),
}

impl std::fmt::Display for ChatFrameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ChatFrameError::TooShort => write!(f, "chat frame too short"),
            ChatFrameError::LengthMismatch => write!(f, "chat frame length mismatch"),
            ChatFrameError::LengthOverflow => write!(f, "chat frame payload too large"),
            ChatFrameError::Io(e) => write!(f, "chat frame io: {e}"),
        }
    }
}

impl std::error::Error for ChatFrameError {}

/// Build a chat frame: `[1B type][2B length BE][payload]`.
pub fn build_chat_frame(message_type: u8, payload: &[u8]) -> Result<Vec<u8>, ChatFrameError> {
    if payload.len() > CHAT_MAX_PAYLOAD_LEN {
        return Err(ChatFrameError::LengthOverflow);
    }
    let mut out = Vec::with_capacity(3 + payload.len());
    out.push(message_type);
    out.extend_from_slice(&(payload.len() as u16).to_be_bytes());
    out.extend_from_slice(payload);
    Ok(out)
}

/// Parse one chat frame from an exact-sized buffer (tests / tooling).
pub fn parse_chat_frame(frame: &[u8]) -> Result<(u8, &[u8]), ChatFrameError> {
    if frame.len() < 3 {
        return Err(ChatFrameError::TooShort);
    }
    let message_type = frame[0];
    let length = u16::from_be_bytes([frame[1], frame[2]]) as usize;
    if length > CHAT_MAX_PAYLOAD_LEN {
        return Err(ChatFrameError::LengthOverflow);
    }
    if frame.len() != 3 + length {
        return Err(ChatFrameError::LengthMismatch);
    }
    Ok((message_type, &frame[3..]))
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ChatWirePayload {
    Message {
        id: String,
        #[serde(default)]
        from_viewer: bool,
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ts_unix_ms: Option<u64>,
    },
    /// Agent notifies viewer/sidecar that the local UI window is up (optional UX signal).
    SidecarReady {},
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RebootNoticeAction {
    Defer,
    RebootNow,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WorkerChatControlPayload {
    RebootNoticeReady {
        notice_id: String,
    },
    RebootNoticeAction {
        notice_id: String,
        action: RebootNoticeAction,
    },
    AiRunnerApprovalRequest {
        approval_id: String,
        requester_label: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        requester_email: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        organization_name: Option<String>,
        device_label: String,
        reason: String,
        expires_at_unix_ms: u64,
        approval_window_expires_at_unix_ms: u64,
    },
    AiRunnerApprovalDecision {
        approval_id: String,
        approved: bool,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ChatAckPayload {
    pub message_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ChatWireErrorPayload {
    pub code: OperationErrorCode,
    pub message: String,
    #[serde(default)]
    pub retryable: bool,
}

#[derive(Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct ChatSessionCapabilitiesHttpResponse {
    pub transports: Vec<String>,
    #[serde(default)]
    pub platform: AgentPlatform,
    #[serde(default)]
    pub features: AgentFeatureCapabilities,
    pub agent_reflex: Option<ReflexAddress>,
    pub agent_host: Option<String>,
    pub agent_local_addrs: Vec<LocalAddr>,
    pub psk_cert_pem: String,
    pub relay_url: Option<String>,
    pub e2e_key: Option<String>,
}

// -----------------------------------------------------------------------------
// Dirty-rect display stream metadata and records
// -----------------------------------------------------------------------------

pub const DISPLAY_STREAM_MODE_LEGACY_CAPTURE: &str = "legacy_capture";
pub const DISPLAY_STREAM_MODE_MODERN_CAPTURE: &str = "modern_capture";
pub const DISPLAY_STREAM_MODE_EXPERIMENTAL_CAPTURE: &str = "experimental_capture";
pub const DISPLAY_STREAM_MODE_SCREENSHOT_ONLY: &str = "screenshot_only";
/// RMM service agent env var and `talos_worker_helper` `--display-processing-mode` use the same name and values.
pub const RMM_DISPLAY_PROCESSING_MODE_ENV: &str = "RMM_DISPLAY_PROCESSING_MODE";
pub const DISPLAY_STREAM_META_TYPE: &str = "display_stream";
pub const DISPLAY_PIXEL_FORMAT_BGRA8: &str = "bgra8";
pub const DISPLAY_PIXEL_FORMAT_H264: &str = "h264";
pub const DISPLAY_COMPRESSION_ZSTD: &str = "zstd";
pub const DISPLAY_COMPRESSION_ANNEX_B: &str = "annex_b";
pub const DISPLAY_COMPRESSION_GPU_TILE_COMMANDS: &str = "gpu_tile_commands";
pub const DISPLAY_COMPRESSION_NONE: &str = "none";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DisplayStreamDescriptor {
    #[serde(default = "display_stream_version_default")]
    pub version: u32,
    pub mode: String,
    pub width: u32,
    pub height: u32,
    pub pixel_format: String,
    pub compression: String,
}

fn display_stream_version_default() -> u32 {
    1
}

impl DisplayStreamDescriptor {
    pub fn modern_capture(width: u32, height: u32) -> Self {
        Self {
            version: 1,
            mode: DISPLAY_STREAM_MODE_MODERN_CAPTURE.to_string(),
            width,
            height,
            pixel_format: DISPLAY_PIXEL_FORMAT_H264.to_string(),
            compression: DISPLAY_COMPRESSION_ANNEX_B.to_string(),
        }
    }

    pub fn experimental_capture(width: u32, height: u32) -> Self {
        Self {
            version: 1,
            mode: DISPLAY_STREAM_MODE_EXPERIMENTAL_CAPTURE.to_string(),
            width,
            height,
            pixel_format: DISPLAY_PIXEL_FORMAT_BGRA8.to_string(),
            compression: DISPLAY_COMPRESSION_GPU_TILE_COMMANDS.to_string(),
        }
    }

    pub fn screenshot_only(width: u32, height: u32) -> Self {
        Self {
            version: 1,
            mode: DISPLAY_STREAM_MODE_SCREENSHOT_ONLY.to_string(),
            width,
            height,
            pixel_format: DISPLAY_PIXEL_FORMAT_BGRA8.to_string(),
            compression: DISPLAY_COMPRESSION_NONE.to_string(),
        }
    }
}

pub const DISPLAY_RECORD_FRAME_BEGIN: u8 = 0x01;
pub const DISPLAY_RECORD_FRAME_END: u8 = 0x03;
pub const DISPLAY_RECORD_KEYFRAME: u8 = 0x04;
pub const DISPLAY_RECORD_MOVE_RECT: u8 = 0x05;
pub const DISPLAY_RECORD_ATLAS_H264: u8 = 0x06;
pub const DISPLAY_RECORD_EXPERIMENTAL_ATLAS_COMMANDS: u8 = 0x07;
pub const DISPLAY_RECORD_EXPERIMENTAL_ATLAS_COMMANDS_CHUNK: u8 = 0x08;
pub const DISPLAY_ATLAS_H264_FLAG_KEYFRAME: u32 = 0x01;
pub const DISPLAY_EXPERIMENTAL_ATLAS_CHUNK_FLAG_PROGRESSIVE: u32 = 0x01;
pub const DISPLAY_EXPERIMENTAL_ATLAS_CHUNK_FLAG_FINAL: u32 = 0x02;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DisplayAtlasRect {
    pub dst_x: u32,
    pub dst_y: u32,
    pub width: u32,
    pub height: u32,
    pub atlas_x: u32,
    pub atlas_y: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DisplayRecord {
    FrameBegin {
        frame_id: u64,
        width: u32,
        height: u32,
    },
    FrameEnd {
        frame_id: u64,
    },
    Keyframe {
        frame_id: u64,
        width: u32,
        height: u32,
        raw_len: u32,
        payload: Vec<u8>,
    },
    MoveRect {
        frame_id: u64,
        src_x: u32,
        src_y: u32,
        dst_x: u32,
        dst_y: u32,
        width: u32,
        height: u32,
    },
    AtlasH264 {
        frame_id: u64,
        flags: u32,
        atlas_width: u32,
        atlas_height: u32,
        rects: Vec<DisplayAtlasRect>,
        payload: Vec<u8>,
    },
    ExperimentalAtlasCommands {
        frame_id: u64,
        atlas_width: u32,
        atlas_height: u32,
        rects: Vec<DisplayAtlasRect>,
        tile_commands: Vec<u8>,
    },
    ExperimentalAtlasCommandsChunk {
        frame_id: u64,
        flags: u32,
        chunk_index: u32,
        chunk_count: u32,
        atlas_width: u32,
        atlas_height: u32,
        rects: Vec<DisplayAtlasRect>,
        tile_commands: Vec<u8>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedDisplayRecord<'a> {
    pub message_type: u8,
    pub payload: &'a [u8],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DisplayRecordParseError {
    TooShort,
    LengthMismatch,
    InvalidPayload,
}

impl std::fmt::Display for DisplayRecordParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DisplayRecordParseError::TooShort => write!(f, "display record too short"),
            DisplayRecordParseError::LengthMismatch => write!(f, "display record length mismatch"),
            DisplayRecordParseError::InvalidPayload => write!(f, "display record payload invalid"),
        }
    }
}

impl std::error::Error for DisplayRecordParseError {}

pub fn build_display_record(message_type: u8, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(1 + 4 + payload.len());
    out.push(message_type);
    out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    out.extend_from_slice(payload);
    out
}

pub fn parse_display_record(
    frame: &[u8],
) -> Result<ParsedDisplayRecord<'_>, DisplayRecordParseError> {
    if frame.len() < 5 {
        return Err(DisplayRecordParseError::TooShort);
    }
    let payload_len = u32::from_le_bytes([frame[1], frame[2], frame[3], frame[4]]) as usize;
    if frame.len() != 5 + payload_len {
        return Err(DisplayRecordParseError::LengthMismatch);
    }
    Ok(ParsedDisplayRecord {
        message_type: frame[0],
        payload: &frame[5..],
    })
}

pub fn build_display_frame_begin(frame_id: u64, width: u32, height: u32) -> Vec<u8> {
    let mut payload = Vec::with_capacity(16);
    payload.extend_from_slice(&frame_id.to_le_bytes());
    payload.extend_from_slice(&width.to_le_bytes());
    payload.extend_from_slice(&height.to_le_bytes());
    build_display_record(DISPLAY_RECORD_FRAME_BEGIN, &payload)
}

pub fn build_display_frame_end(frame_id: u64) -> Vec<u8> {
    build_display_record(DISPLAY_RECORD_FRAME_END, &frame_id.to_le_bytes())
}

pub fn build_display_move_rect(
    frame_id: u64,
    src_x: u32,
    src_y: u32,
    dst_x: u32,
    dst_y: u32,
    width: u32,
    height: u32,
) -> Vec<u8> {
    let mut payload = Vec::with_capacity(32);
    payload.extend_from_slice(&frame_id.to_le_bytes());
    payload.extend_from_slice(&src_x.to_le_bytes());
    payload.extend_from_slice(&src_y.to_le_bytes());
    payload.extend_from_slice(&dst_x.to_le_bytes());
    payload.extend_from_slice(&dst_y.to_le_bytes());
    payload.extend_from_slice(&width.to_le_bytes());
    payload.extend_from_slice(&height.to_le_bytes());
    build_display_record(DISPLAY_RECORD_MOVE_RECT, &payload)
}

pub fn build_display_keyframe(
    frame_id: u64,
    width: u32,
    height: u32,
    raw_len: u32,
    payload: &[u8],
) -> Vec<u8> {
    let mut body = Vec::with_capacity(20 + payload.len());
    body.extend_from_slice(&frame_id.to_le_bytes());
    body.extend_from_slice(&width.to_le_bytes());
    body.extend_from_slice(&height.to_le_bytes());
    body.extend_from_slice(&raw_len.to_le_bytes());
    body.extend_from_slice(payload);
    build_display_record(DISPLAY_RECORD_KEYFRAME, &body)
}

pub fn build_display_atlas_h264(
    frame_id: u64,
    flags: u32,
    atlas_width: u32,
    atlas_height: u32,
    rects: &[DisplayAtlasRect],
    payload: &[u8],
) -> Vec<u8> {
    let mut body = Vec::with_capacity(24 + rects.len() * 24 + payload.len());
    body.extend_from_slice(&frame_id.to_le_bytes());
    body.extend_from_slice(&flags.to_le_bytes());
    body.extend_from_slice(&atlas_width.to_le_bytes());
    body.extend_from_slice(&atlas_height.to_le_bytes());
    body.extend_from_slice(&(rects.len() as u32).to_le_bytes());
    for rect in rects {
        body.extend_from_slice(&rect.dst_x.to_le_bytes());
        body.extend_from_slice(&rect.dst_y.to_le_bytes());
        body.extend_from_slice(&rect.width.to_le_bytes());
        body.extend_from_slice(&rect.height.to_le_bytes());
        body.extend_from_slice(&rect.atlas_x.to_le_bytes());
        body.extend_from_slice(&rect.atlas_y.to_le_bytes());
    }
    body.extend_from_slice(payload);
    build_display_record(DISPLAY_RECORD_ATLAS_H264, &body)
}

pub fn build_display_experimental_atlas_commands(
    frame_id: u64,
    atlas_width: u32,
    atlas_height: u32,
    rects: &[DisplayAtlasRect],
    tile_commands: &[u8],
) -> Vec<u8> {
    let mut body = Vec::with_capacity(24 + rects.len() * 24 + tile_commands.len());
    body.extend_from_slice(&frame_id.to_le_bytes());
    body.extend_from_slice(&atlas_width.to_le_bytes());
    body.extend_from_slice(&atlas_height.to_le_bytes());
    body.extend_from_slice(&(rects.len() as u32).to_le_bytes());
    body.extend_from_slice(&(tile_commands.len() as u32).to_le_bytes());
    for rect in rects {
        body.extend_from_slice(&rect.dst_x.to_le_bytes());
        body.extend_from_slice(&rect.dst_y.to_le_bytes());
        body.extend_from_slice(&rect.width.to_le_bytes());
        body.extend_from_slice(&rect.height.to_le_bytes());
        body.extend_from_slice(&rect.atlas_x.to_le_bytes());
        body.extend_from_slice(&rect.atlas_y.to_le_bytes());
    }
    body.extend_from_slice(tile_commands);
    build_display_record(DISPLAY_RECORD_EXPERIMENTAL_ATLAS_COMMANDS, &body)
}

pub fn build_display_experimental_atlas_commands_chunk(
    frame_id: u64,
    flags: u32,
    chunk_index: u32,
    chunk_count: u32,
    atlas_width: u32,
    atlas_height: u32,
    rects: &[DisplayAtlasRect],
    tile_commands: &[u8],
) -> Vec<u8> {
    let mut body = Vec::with_capacity(36 + rects.len() * 24 + tile_commands.len());
    body.extend_from_slice(&frame_id.to_le_bytes());
    body.extend_from_slice(&flags.to_le_bytes());
    body.extend_from_slice(&chunk_index.to_le_bytes());
    body.extend_from_slice(&chunk_count.to_le_bytes());
    body.extend_from_slice(&atlas_width.to_le_bytes());
    body.extend_from_slice(&atlas_height.to_le_bytes());
    body.extend_from_slice(&(rects.len() as u32).to_le_bytes());
    body.extend_from_slice(&(tile_commands.len() as u32).to_le_bytes());
    for rect in rects {
        body.extend_from_slice(&rect.dst_x.to_le_bytes());
        body.extend_from_slice(&rect.dst_y.to_le_bytes());
        body.extend_from_slice(&rect.width.to_le_bytes());
        body.extend_from_slice(&rect.height.to_le_bytes());
        body.extend_from_slice(&rect.atlas_x.to_le_bytes());
        body.extend_from_slice(&rect.atlas_y.to_le_bytes());
    }
    body.extend_from_slice(tile_commands);
    build_display_record(DISPLAY_RECORD_EXPERIMENTAL_ATLAS_COMMANDS_CHUNK, &body)
}

pub fn decode_display_record(frame: &[u8]) -> Result<DisplayRecord, DisplayRecordParseError> {
    let parsed = parse_display_record(frame)?;
    match parsed.message_type {
        DISPLAY_RECORD_FRAME_BEGIN => {
            if parsed.payload.len() != 16 {
                return Err(DisplayRecordParseError::InvalidPayload);
            }
            Ok(DisplayRecord::FrameBegin {
                frame_id: u64::from_le_bytes(parsed.payload[0..8].try_into().unwrap()),
                width: u32::from_le_bytes(parsed.payload[8..12].try_into().unwrap()),
                height: u32::from_le_bytes(parsed.payload[12..16].try_into().unwrap()),
            })
        }
        DISPLAY_RECORD_FRAME_END => {
            if parsed.payload.len() != 8 {
                return Err(DisplayRecordParseError::InvalidPayload);
            }
            Ok(DisplayRecord::FrameEnd {
                frame_id: u64::from_le_bytes(parsed.payload[0..8].try_into().unwrap()),
            })
        }
        DISPLAY_RECORD_KEYFRAME => {
            if parsed.payload.len() < 20 {
                return Err(DisplayRecordParseError::InvalidPayload);
            }
            Ok(DisplayRecord::Keyframe {
                frame_id: u64::from_le_bytes(parsed.payload[0..8].try_into().unwrap()),
                width: u32::from_le_bytes(parsed.payload[8..12].try_into().unwrap()),
                height: u32::from_le_bytes(parsed.payload[12..16].try_into().unwrap()),
                raw_len: u32::from_le_bytes(parsed.payload[16..20].try_into().unwrap()),
                payload: parsed.payload[20..].to_vec(),
            })
        }
        DISPLAY_RECORD_MOVE_RECT => {
            if parsed.payload.len() != 32 {
                return Err(DisplayRecordParseError::InvalidPayload);
            }
            Ok(DisplayRecord::MoveRect {
                frame_id: u64::from_le_bytes(parsed.payload[0..8].try_into().unwrap()),
                src_x: u32::from_le_bytes(parsed.payload[8..12].try_into().unwrap()),
                src_y: u32::from_le_bytes(parsed.payload[12..16].try_into().unwrap()),
                dst_x: u32::from_le_bytes(parsed.payload[16..20].try_into().unwrap()),
                dst_y: u32::from_le_bytes(parsed.payload[20..24].try_into().unwrap()),
                width: u32::from_le_bytes(parsed.payload[24..28].try_into().unwrap()),
                height: u32::from_le_bytes(parsed.payload[28..32].try_into().unwrap()),
            })
        }
        DISPLAY_RECORD_ATLAS_H264 => {
            if parsed.payload.len() < 24 {
                return Err(DisplayRecordParseError::InvalidPayload);
            }
            let frame_id = u64::from_le_bytes(parsed.payload[0..8].try_into().unwrap());
            let flags = u32::from_le_bytes(parsed.payload[8..12].try_into().unwrap());
            let atlas_width = u32::from_le_bytes(parsed.payload[12..16].try_into().unwrap());
            let atlas_height = u32::from_le_bytes(parsed.payload[16..20].try_into().unwrap());
            let rect_count =
                u32::from_le_bytes(parsed.payload[20..24].try_into().unwrap()) as usize;
            if atlas_width == 0 || atlas_height == 0 || rect_count == 0 {
                return Err(DisplayRecordParseError::InvalidPayload);
            }
            let rect_table_len = rect_count
                .checked_mul(24)
                .ok_or(DisplayRecordParseError::InvalidPayload)?;
            let payload_offset = 24usize
                .checked_add(rect_table_len)
                .ok_or(DisplayRecordParseError::InvalidPayload)?;
            if parsed.payload.len() <= payload_offset {
                return Err(DisplayRecordParseError::InvalidPayload);
            }
            let mut rects = Vec::with_capacity(rect_count);
            let mut cursor = 24usize;
            for _ in 0..rect_count {
                let rect = DisplayAtlasRect {
                    dst_x: u32::from_le_bytes(
                        parsed.payload[cursor..cursor + 4].try_into().unwrap(),
                    ),
                    dst_y: u32::from_le_bytes(
                        parsed.payload[cursor + 4..cursor + 8].try_into().unwrap(),
                    ),
                    width: u32::from_le_bytes(
                        parsed.payload[cursor + 8..cursor + 12].try_into().unwrap(),
                    ),
                    height: u32::from_le_bytes(
                        parsed.payload[cursor + 12..cursor + 16].try_into().unwrap(),
                    ),
                    atlas_x: u32::from_le_bytes(
                        parsed.payload[cursor + 16..cursor + 20].try_into().unwrap(),
                    ),
                    atlas_y: u32::from_le_bytes(
                        parsed.payload[cursor + 20..cursor + 24].try_into().unwrap(),
                    ),
                };
                if rect.width == 0
                    || rect.height == 0
                    || rect.atlas_x.checked_add(rect.width).is_none()
                    || rect.atlas_y.checked_add(rect.height).is_none()
                    || rect.atlas_x + rect.width > atlas_width
                    || rect.atlas_y + rect.height > atlas_height
                {
                    return Err(DisplayRecordParseError::InvalidPayload);
                }
                rects.push(rect);
                cursor += 24;
            }
            Ok(DisplayRecord::AtlasH264 {
                frame_id,
                flags,
                atlas_width,
                atlas_height,
                rects,
                payload: parsed.payload[payload_offset..].to_vec(),
            })
        }
        DISPLAY_RECORD_EXPERIMENTAL_ATLAS_COMMANDS => {
            if parsed.payload.len() < 24 {
                return Err(DisplayRecordParseError::InvalidPayload);
            }
            let frame_id = u64::from_le_bytes(parsed.payload[0..8].try_into().unwrap());
            let atlas_width = u32::from_le_bytes(parsed.payload[8..12].try_into().unwrap());
            let atlas_height = u32::from_le_bytes(parsed.payload[12..16].try_into().unwrap());
            let rect_count =
                u32::from_le_bytes(parsed.payload[16..20].try_into().unwrap()) as usize;
            let tile_commands_len =
                u32::from_le_bytes(parsed.payload[20..24].try_into().unwrap()) as usize;
            if atlas_width == 0 || atlas_height == 0 {
                return Err(DisplayRecordParseError::InvalidPayload);
            }
            let rect_table_len = rect_count
                .checked_mul(24)
                .ok_or(DisplayRecordParseError::InvalidPayload)?;
            let commands_offset = 24usize
                .checked_add(rect_table_len)
                .ok_or(DisplayRecordParseError::InvalidPayload)?;
            let expected_len = commands_offset
                .checked_add(tile_commands_len)
                .ok_or(DisplayRecordParseError::InvalidPayload)?;
            if parsed.payload.len() != expected_len {
                return Err(DisplayRecordParseError::InvalidPayload);
            }
            let mut rects = Vec::with_capacity(rect_count);
            let mut cursor = 24usize;
            for _ in 0..rect_count {
                let rect = DisplayAtlasRect {
                    dst_x: u32::from_le_bytes(
                        parsed.payload[cursor..cursor + 4].try_into().unwrap(),
                    ),
                    dst_y: u32::from_le_bytes(
                        parsed.payload[cursor + 4..cursor + 8].try_into().unwrap(),
                    ),
                    width: u32::from_le_bytes(
                        parsed.payload[cursor + 8..cursor + 12].try_into().unwrap(),
                    ),
                    height: u32::from_le_bytes(
                        parsed.payload[cursor + 12..cursor + 16].try_into().unwrap(),
                    ),
                    atlas_x: u32::from_le_bytes(
                        parsed.payload[cursor + 16..cursor + 20].try_into().unwrap(),
                    ),
                    atlas_y: u32::from_le_bytes(
                        parsed.payload[cursor + 20..cursor + 24].try_into().unwrap(),
                    ),
                };
                if rect.width == 0
                    || rect.height == 0
                    || rect.atlas_x.checked_add(rect.width).is_none()
                    || rect.atlas_y.checked_add(rect.height).is_none()
                    || rect.atlas_x + rect.width > atlas_width
                    || rect.atlas_y + rect.height > atlas_height
                {
                    return Err(DisplayRecordParseError::InvalidPayload);
                }
                rects.push(rect);
                cursor += 24;
            }
            Ok(DisplayRecord::ExperimentalAtlasCommands {
                frame_id,
                atlas_width,
                atlas_height,
                rects,
                tile_commands: parsed.payload[commands_offset..expected_len].to_vec(),
            })
        }
        DISPLAY_RECORD_EXPERIMENTAL_ATLAS_COMMANDS_CHUNK => {
            if parsed.payload.len() < 36 {
                return Err(DisplayRecordParseError::InvalidPayload);
            }
            let frame_id = u64::from_le_bytes(parsed.payload[0..8].try_into().unwrap());
            let flags = u32::from_le_bytes(parsed.payload[8..12].try_into().unwrap());
            let chunk_index = u32::from_le_bytes(parsed.payload[12..16].try_into().unwrap());
            let chunk_count = u32::from_le_bytes(parsed.payload[16..20].try_into().unwrap());
            let atlas_width = u32::from_le_bytes(parsed.payload[20..24].try_into().unwrap());
            let atlas_height = u32::from_le_bytes(parsed.payload[24..28].try_into().unwrap());
            let rect_count =
                u32::from_le_bytes(parsed.payload[28..32].try_into().unwrap()) as usize;
            let tile_commands_len =
                u32::from_le_bytes(parsed.payload[32..36].try_into().unwrap()) as usize;
            if atlas_width == 0
                || atlas_height == 0
                || chunk_count == 0
                || chunk_index >= chunk_count
            {
                return Err(DisplayRecordParseError::InvalidPayload);
            }
            let rect_table_len = rect_count
                .checked_mul(24)
                .ok_or(DisplayRecordParseError::InvalidPayload)?;
            let commands_offset = 36usize
                .checked_add(rect_table_len)
                .ok_or(DisplayRecordParseError::InvalidPayload)?;
            let expected_len = commands_offset
                .checked_add(tile_commands_len)
                .ok_or(DisplayRecordParseError::InvalidPayload)?;
            if parsed.payload.len() != expected_len {
                return Err(DisplayRecordParseError::InvalidPayload);
            }
            let mut rects = Vec::with_capacity(rect_count);
            let mut cursor = 36usize;
            for _ in 0..rect_count {
                let rect = DisplayAtlasRect {
                    dst_x: u32::from_le_bytes(
                        parsed.payload[cursor..cursor + 4].try_into().unwrap(),
                    ),
                    dst_y: u32::from_le_bytes(
                        parsed.payload[cursor + 4..cursor + 8].try_into().unwrap(),
                    ),
                    width: u32::from_le_bytes(
                        parsed.payload[cursor + 8..cursor + 12].try_into().unwrap(),
                    ),
                    height: u32::from_le_bytes(
                        parsed.payload[cursor + 12..cursor + 16].try_into().unwrap(),
                    ),
                    atlas_x: u32::from_le_bytes(
                        parsed.payload[cursor + 16..cursor + 20].try_into().unwrap(),
                    ),
                    atlas_y: u32::from_le_bytes(
                        parsed.payload[cursor + 20..cursor + 24].try_into().unwrap(),
                    ),
                };
                if rect.width == 0
                    || rect.height == 0
                    || rect.atlas_x.checked_add(rect.width).is_none()
                    || rect.atlas_y.checked_add(rect.height).is_none()
                    || rect.atlas_x + rect.width > atlas_width
                    || rect.atlas_y + rect.height > atlas_height
                {
                    return Err(DisplayRecordParseError::InvalidPayload);
                }
                rects.push(rect);
                cursor += 24;
            }
            Ok(DisplayRecord::ExperimentalAtlasCommandsChunk {
                frame_id,
                flags,
                chunk_index,
                chunk_count,
                atlas_width,
                atlas_height,
                rects,
                tile_commands: parsed.payload[commands_offset..expected_len].to_vec(),
            })
        }
        _ => Err(DisplayRecordParseError::InvalidPayload),
    }
}

// -----------------------------------------------------------------------------
// File transfer framed stream protocol (viewer ↔ agent)
// -----------------------------------------------------------------------------

/// Wire frame: `[1B type][4B payload length BE][payload]`.
pub const FILE_TRANSFER_MAX_PAYLOAD_LEN: usize = 4 * 1024 * 1024;

/// JSON-encoded command/response payload.
pub const FILE_TRANSFER_MSG_JSON: u8 = 0x01;
/// Raw binary payload (file chunk).
pub const FILE_TRANSFER_MSG_DATA: u8 = 0x02;
/// End-of-transfer marker (no payload expected).
pub const FILE_TRANSFER_MSG_FINISH: u8 = 0x03;
/// Error payload (UTF-8 string).
pub const FILE_TRANSFER_MSG_ERROR: u8 = 0x04;

pub const FILE_TRANSFER_DEFAULT_ZIP_THRESHOLD_FILES: u32 = 50;
pub const FILE_TRANSFER_DEFAULT_ZIP_THRESHOLD_BYTES: u64 = 128 * 1024 * 1024;
/// Use stored (no compression) archives for very large batch selections to
/// reduce CPU-heavy pre-transfer delay.
pub const FILE_TRANSFER_STORE_ARCHIVE_THRESHOLD_FILES: u32 = 250;
pub const FILE_TRANSFER_STORE_ARCHIVE_THRESHOLD_BYTES: u64 = 512 * 1024 * 1024;
pub const FILE_TRANSFER_DEFAULT_CHUNK_BYTES: u32 = 512 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct OperationError {
    pub code: OperationErrorCode,
    pub message: String,
    #[serde(default)]
    pub retryable: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct FileTransferFrame<'a> {
    pub message_type: u8,
    pub payload: &'a [u8],
}

#[derive(Debug, Clone)]
pub enum FileTransferFrameError {
    TooShort,
    LengthOverflow,
    LengthMismatch,
    PayloadTooLarge,
}

impl std::fmt::Display for FileTransferFrameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FileTransferFrameError::TooShort => write!(f, "file transfer frame too short"),
            FileTransferFrameError::LengthOverflow => {
                write!(f, "file transfer frame length overflow")
            }
            FileTransferFrameError::LengthMismatch => {
                write!(f, "file transfer frame length mismatch")
            }
            FileTransferFrameError::PayloadTooLarge => write!(f, "file transfer payload too large"),
        }
    }
}

impl std::error::Error for FileTransferFrameError {}

pub fn parse_file_transfer_frame(
    frame: &[u8],
) -> Result<FileTransferFrame<'_>, FileTransferFrameError> {
    if frame.len() < 5 {
        return Err(FileTransferFrameError::TooShort);
    }
    let payload_len = u32::from_be_bytes([frame[1], frame[2], frame[3], frame[4]]) as usize;
    if payload_len > FILE_TRANSFER_MAX_PAYLOAD_LEN {
        return Err(FileTransferFrameError::LengthOverflow);
    }
    if frame.len() != 5 + payload_len {
        return Err(FileTransferFrameError::LengthMismatch);
    }
    Ok(FileTransferFrame {
        message_type: frame[0],
        payload: &frame[5..],
    })
}

pub fn build_file_transfer_frame(
    message_type: u8,
    payload: &[u8],
) -> Result<Vec<u8>, FileTransferFrameError> {
    if payload.len() > FILE_TRANSFER_MAX_PAYLOAD_LEN {
        return Err(FileTransferFrameError::PayloadTooLarge);
    }
    let mut out = Vec::with_capacity(5 + payload.len());
    out.push(message_type);
    out.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    out.extend_from_slice(payload);
    Ok(out)
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FileTransferConflictMode {
    Prompt,
    Skip,
    Overwrite,
    Rename,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FileTransferEntry {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub size_bytes: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modified_unix_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum FileTransferRequest {
    ListDir {
        path: String,
    },
    Download {
        transfer_id: String,
        paths: Vec<String>,
        #[serde(default)]
        resume_offset: u64,
    },
    Rename {
        from_path: String,
        to_path: String,
    },
    Delete {
        path: String,
        recursive: bool,
    },
    Upload {
        transfer_id: String,
        destination_path: String,
        file_name: String,
        is_archive: bool,
        extract_archive: bool,
        conflict_mode: FileTransferConflictMode,
        expected_size_bytes: u64,
        #[serde(default)]
        resume_offset: u64,
    },
    Cancel {
        transfer_id: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum FileTransferResponse {
    Progress {
        files_done: u64,
        files_total: u64,
        bytes_done: u64,
        bytes_total: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        phase: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message: Option<String>,
    },
    ListDirResult {
        path: String,
        entries: Vec<FileTransferEntry>,
    },
    DownloadReady {
        transfer_id: String,
        file_name: String,
        size_bytes: u64,
        is_archive: bool,
        #[serde(default)]
        resume_offset: u64,
    },
    Ok {},
    UploadReady {
        transfer_id: String,
        #[serde(default)]
        resume_offset: u64,
    },
    TransferComplete {
        transfer_id: String,
        bytes_transferred: u64,
        extracted_entries: u32,
    },
    Conflict {
        path: String,
        message: String,
    },
    Error {
        code: OperationErrorCode,
        message: String,
        #[serde(default)]
        retryable: bool,
    },
}

// -----------------------------------------------------------------------------
// HTTP response type (server → viewer)
// -----------------------------------------------------------------------------

#[derive(Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct SessionCapabilitiesHttpResponse {
    pub codecs: Vec<String>,
    pub encoding: String,
    pub transports: Vec<String>,
    #[serde(default)]
    pub platform: AgentPlatform,
    #[serde(default)]
    pub features: AgentFeatureCapabilities,
    #[serde(default)]
    pub display_profiles: Vec<RemoteDesktopDisplayProfile>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected_display_profile: Option<String>,
    pub agent_reflex: Option<ReflexAddress>,
    pub agent_host: Option<String>,
    pub agent_local_addrs: Vec<LocalAddr>,
    pub psk_cert_pem: String,
    pub relay_url: Option<String>,
    pub e2e_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_hostname: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_os: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_version: Option<String>,
}

#[derive(Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct FileTransferSessionCapabilitiesHttpResponse {
    pub transports: Vec<String>,
    #[serde(default)]
    pub platform: AgentPlatform,
    #[serde(default)]
    pub features: AgentFeatureCapabilities,
    pub agent_reflex: Option<ReflexAddress>,
    pub agent_host: Option<String>,
    pub agent_local_addrs: Vec<LocalAddr>,
    pub psk_cert_pem: String,
    pub relay_url: Option<String>,
    pub e2e_key: Option<String>,
    pub zip_threshold_files: u32,
    #[cfg_attr(feature = "typescript", ts(type = "number"))]
    pub zip_threshold_bytes: u64,
    pub max_chunk_bytes: u32,
}

#[derive(Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct RegistrySessionCapabilitiesHttpResponse {
    pub transports: Vec<String>,
    #[serde(default)]
    pub platform: AgentPlatform,
    #[serde(default)]
    pub features: AgentFeatureCapabilities,
    pub agent_reflex: Option<ReflexAddress>,
    pub agent_host: Option<String>,
    pub agent_local_addrs: Vec<LocalAddr>,
    pub psk_cert_pem: String,
    pub relay_url: Option<String>,
    pub e2e_key: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::{
        build_chat_frame, build_control_frame, build_display_atlas_h264,
        build_display_experimental_atlas_commands, build_display_experimental_atlas_commands_chunk,
        build_display_move_rect, build_file_transfer_frame, build_shell_exit_payload,
        build_shell_frame, build_shell_resize_payload, decode_display_record, parse_chat_frame,
        parse_control_frame, parse_file_transfer_frame, parse_shell_exit_payload,
        parse_shell_resize_payload, ChatAckPayload, ChatWireErrorPayload, ChatWirePayload,
        DisplayAtlasRect, DisplayRecord, DisplayRecordParseError, FileTransferConflictMode,
        FileTransferRequest, FileTransferResponse, OperationErrorCode, RebootNoticeAction,
        RegistryHive, RegistryRequest, RegistryResponse, RegistryResponseEnvelope,
        RegistryValueData, RegistryValueEntry, RemoteDesktopCapabilities,
        RemoteDesktopDisplayProfile, StunServerConfigError, WorkerChatControlPayload, CHAT_MSG_ACK,
        CHAT_MSG_AUTH, CHAT_MSG_CONTROL, CHAT_MSG_ERROR, CHAT_MSG_TEXT,
        CONTROL_PAYLOAD_CAPTURE_OUTPUT_INDEX_LEN, CONTROL_PAYLOAD_MOUSE_DOUBLE_CLICK_LEN,
        CONTROL_PAYLOAD_SESSION_ID_LEN, CONTROL_PAYLOAD_STREAM_BITRATE_LEN,
        CONTROL_TYPE_CAPTURE_OUTPUT_SWITCH, CONTROL_TYPE_MOUSE_DOUBLE_CLICK,
        CONTROL_TYPE_SESSION_LOGOFF, CONTROL_TYPE_SESSION_SWITCH, CONTROL_TYPE_STREAM_BITRATE,
        DISPLAY_ATLAS_H264_FLAG_KEYFRAME, DISPLAY_EXPERIMENTAL_ATLAS_CHUNK_FLAG_FINAL,
        DISPLAY_EXPERIMENTAL_ATLAS_CHUNK_FLAG_PROGRESSIVE, FILE_TRANSFER_MSG_DATA,
        FILE_TRANSFER_MSG_JSON, REGISTRY_META_MESSAGE_TYPE, REMOTE_DESKTOP_CODEC_H264,
        REMOTE_DESKTOP_CODEC_SCREENSHOT_BGRA, REMOTE_DESKTOP_COMPRESSION_ANNEX_B,
        REMOTE_DESKTOP_COMPRESSION_NONE, REMOTE_DESKTOP_PROFILE_MODERN_GPU,
        REMOTE_DESKTOP_PROFILE_SCREENSHOT_ONLY, REMOTE_DESKTOP_PROTOCOL_EXPERIMENTAL_DISPLAY_DELTA,
        REMOTE_DESKTOP_PROTOCOL_MODERN_DISPLAY_DELTA, REMOTE_DESKTOP_PROTOCOL_SCREENSHOT_ONLY,
        SHELL_MSG_AUTH, SHELL_MSG_INPUT, SHELL_MSG_OUTPUT, SHELL_MSG_RESIZE,
    };

    #[test]
    fn stun_is_disabled_when_configuration_is_absent_or_blank() {
        assert_eq!(super::parse_stun_server(None), Ok(None));
        assert_eq!(super::parse_stun_server(Some("  \t")), Ok(None));
    }

    #[test]
    fn stun_accepts_only_explicit_hostname_or_ipv4_endpoints() {
        assert_eq!(
            super::parse_stun_server(Some(" stun.community.example:3478 ")),
            Ok(Some("stun.community.example:3478".to_string()))
        );
        assert_eq!(
            super::parse_stun_server(Some("192.0.2.10:19302")),
            Ok(Some("192.0.2.10:19302".to_string()))
        );

        for invalid in [
            "https://stun.example:3478",
            "user@stun.example:3478",
            "stun.example:0",
            "stun.example:65536",
            "stun.example/path:3478",
            "[2001:db8::1]:3478",
            "-stun.example:3478",
        ] {
            assert!(
                super::parse_stun_server(Some(invalid)).is_err(),
                "unexpectedly accepted {invalid}"
            );
        }
        assert_eq!(
            super::parse_stun_server(Some("stun.example")),
            Err(StunServerConfigError::MissingPort)
        );
    }

    #[test]
    fn remote_desktop_display_profiles_serialize_explicit_protocols() {
        let capabilities = RemoteDesktopCapabilities {
            codecs: vec!["h264".to_string(), "vp8".to_string()],
            encoding: "software".to_string(),
            transports: vec!["quic".to_string(), "relay".to_string()],
            platform: super::AgentPlatform::Windows,
            features: super::AgentFeatureCapabilities::windows(),
            display_profiles: vec![
                RemoteDesktopDisplayProfile::modern_gpu(),
                RemoteDesktopDisplayProfile::legacy(),
            ],
            selected_display_profile: Some(REMOTE_DESKTOP_PROFILE_MODERN_GPU.to_string()),
        };

        let value = serde_json::to_value(&capabilities).expect("serialize capabilities");
        assert_eq!(
            value["displayProfiles"][0]["protocol"],
            REMOTE_DESKTOP_PROTOCOL_MODERN_DISPLAY_DELTA
        );
        assert_eq!(
            value["displayProfiles"][0]["codec"],
            REMOTE_DESKTOP_CODEC_H264
        );
        assert_eq!(
            value["displayProfiles"][0]["compression"],
            REMOTE_DESKTOP_COMPRESSION_ANNEX_B
        );
        assert_eq!(
            value["selectedDisplayProfile"],
            REMOTE_DESKTOP_PROFILE_MODERN_GPU
        );

        let experimental = RemoteDesktopDisplayProfile::experimental();
        assert_eq!(
            experimental.protocol,
            REMOTE_DESKTOP_PROTOCOL_EXPERIMENTAL_DISPLAY_DELTA
        );

        let screenshot = RemoteDesktopDisplayProfile::screenshot_only();
        assert_eq!(screenshot.id, REMOTE_DESKTOP_PROFILE_SCREENSHOT_ONLY);
        assert_eq!(screenshot.protocol, REMOTE_DESKTOP_PROTOCOL_SCREENSHOT_ONLY);
        assert_eq!(screenshot.codec, REMOTE_DESKTOP_CODEC_SCREENSHOT_BGRA);
        assert_eq!(screenshot.compression, REMOTE_DESKTOP_COMPRESSION_NONE);
    }

    #[test]
    fn display_stream_descriptor_supports_screenshot_only() {
        let descriptor = super::DisplayStreamDescriptor::screenshot_only(1920, 1080);
        assert_eq!(descriptor.mode, super::DISPLAY_STREAM_MODE_SCREENSHOT_ONLY);
        assert_eq!(descriptor.width, 1920);
        assert_eq!(descriptor.height, 1080);
        assert_eq!(descriptor.pixel_format, super::DISPLAY_PIXEL_FORMAT_BGRA8);
        assert_eq!(descriptor.compression, super::DISPLAY_COMPRESSION_NONE);
    }

    #[test]
    fn macos_feature_defaults_include_system_shell_file_transfer_and_chat() {
        let features = super::AgentFeatureCapabilities::macos();
        assert!(features.remote_desktop);
        assert!(features.system_shell);
        assert!(features.file_transfer);
        assert!(!features.remote_registry);
        assert!(features.chat);
        assert!(features.system_info);
        assert_eq!(
            super::AgentFeatureCapabilities::for_platform(super::AgentPlatform::Macos),
            features
        );
    }

    #[test]
    fn session_switch_frame_roundtrip() {
        let session_id: u32 = 3;
        let payload = session_id.to_be_bytes();
        let frame = build_control_frame(CONTROL_TYPE_SESSION_SWITCH, &payload)
            .expect("build session switch frame");
        let parsed = parse_control_frame(&frame).expect("parse session switch frame");
        assert_eq!(parsed.message_type, CONTROL_TYPE_SESSION_SWITCH);
        assert_eq!(parsed.payload.len(), CONTROL_PAYLOAD_SESSION_ID_LEN);
        let parsed_session_id = u32::from_be_bytes([
            parsed.payload[0],
            parsed.payload[1],
            parsed.payload[2],
            parsed.payload[3],
        ]);
        assert_eq!(parsed_session_id, session_id);
    }

    #[test]
    fn capture_output_switch_frame_roundtrip() {
        let index: u32 = 2;
        let payload = index.to_be_bytes();
        let frame = build_control_frame(CONTROL_TYPE_CAPTURE_OUTPUT_SWITCH, &payload)
            .expect("build capture output switch frame");
        let parsed = parse_control_frame(&frame).expect("parse capture output switch frame");
        assert_eq!(parsed.message_type, CONTROL_TYPE_CAPTURE_OUTPUT_SWITCH);
        assert_eq!(
            parsed.payload.len(),
            CONTROL_PAYLOAD_CAPTURE_OUTPUT_INDEX_LEN
        );
        let parsed_index = u32::from_be_bytes([
            parsed.payload[0],
            parsed.payload[1],
            parsed.payload[2],
            parsed.payload[3],
        ]);
        assert_eq!(parsed_index, index);
    }

    #[test]
    fn stream_bitrate_frame_roundtrip() {
        let kbps: u32 = 20_000;
        let payload = kbps.to_be_bytes();
        let frame = build_control_frame(CONTROL_TYPE_STREAM_BITRATE, &payload)
            .expect("build stream bitrate frame");
        let parsed = parse_control_frame(&frame).expect("parse stream bitrate frame");
        assert_eq!(parsed.message_type, CONTROL_TYPE_STREAM_BITRATE);
        assert_eq!(parsed.payload.len(), CONTROL_PAYLOAD_STREAM_BITRATE_LEN);
        let parsed_kbps = u32::from_be_bytes([
            parsed.payload[0],
            parsed.payload[1],
            parsed.payload[2],
            parsed.payload[3],
        ]);
        assert_eq!(parsed_kbps, kbps);
    }

    #[test]
    fn mouse_double_click_frame_roundtrip() {
        let mut payload = Vec::with_capacity(CONTROL_PAYLOAD_MOUSE_DOUBLE_CLICK_LEN);
        payload.push(0);
        payload.extend_from_slice(&32_768u32.to_be_bytes());
        payload.extend_from_slice(&16_384u32.to_be_bytes());
        let frame = build_control_frame(CONTROL_TYPE_MOUSE_DOUBLE_CLICK, &payload)
            .expect("build mouse double-click frame");
        let parsed = parse_control_frame(&frame).expect("parse mouse double-click frame");
        assert_eq!(parsed.message_type, CONTROL_TYPE_MOUSE_DOUBLE_CLICK);
        assert_eq!(parsed.payload.len(), CONTROL_PAYLOAD_MOUSE_DOUBLE_CLICK_LEN);
        assert_eq!(parsed.payload, payload.as_slice());
    }

    #[test]
    fn session_logoff_frame_roundtrip() {
        let session_id: u32 = 9;
        let payload = session_id.to_be_bytes();
        let frame = build_control_frame(CONTROL_TYPE_SESSION_LOGOFF, &payload)
            .expect("build session logoff frame");
        let parsed = parse_control_frame(&frame).expect("parse session logoff frame");
        assert_eq!(parsed.message_type, CONTROL_TYPE_SESSION_LOGOFF);
        assert_eq!(parsed.payload.len(), CONTROL_PAYLOAD_SESSION_ID_LEN);
        let parsed_session_id = u32::from_be_bytes([
            parsed.payload[0],
            parsed.payload[1],
            parsed.payload[2],
            parsed.payload[3],
        ]);
        assert_eq!(parsed_session_id, session_id);
    }

    #[test]
    fn shell_frame_build_and_structure() {
        let data = b"hello";
        let frame = build_shell_frame(SHELL_MSG_INPUT, data).expect("build shell input frame");
        assert_eq!(frame.len(), 3 + data.len());
        assert_eq!(frame[0], SHELL_MSG_INPUT);
        let len = u16::from_be_bytes([frame[1], frame[2]]) as usize;
        assert_eq!(len, data.len());
        assert_eq!(&frame[3..], data);
    }

    #[test]
    fn shell_auth_frame() {
        let token = b"secret-token-123";
        let frame = build_shell_frame(SHELL_MSG_AUTH, token).expect("build auth frame");
        assert_eq!(frame[0], SHELL_MSG_AUTH);
        let len = u16::from_be_bytes([frame[1], frame[2]]) as usize;
        assert_eq!(&frame[3..3 + len], token);
    }

    #[test]
    fn shell_output_frame() {
        let output = b"\x1b[32mPS C:\\>\x1b[0m ";
        let frame = build_shell_frame(SHELL_MSG_OUTPUT, output).expect("build output frame");
        assert_eq!(frame[0], SHELL_MSG_OUTPUT);
        assert_eq!(&frame[3..], output);
    }

    #[test]
    fn shell_resize_roundtrip() {
        let payload = build_shell_resize_payload(120, 40);
        let (cols, rows) = parse_shell_resize_payload(&payload).expect("parse resize");
        assert_eq!(cols, 120);
        assert_eq!(rows, 40);
    }

    #[test]
    fn shell_resize_frame_roundtrip() {
        let resize_payload = build_shell_resize_payload(200, 50);
        let frame =
            build_shell_frame(SHELL_MSG_RESIZE, &resize_payload).expect("build resize frame");
        assert_eq!(frame[0], SHELL_MSG_RESIZE);
        let len = u16::from_be_bytes([frame[1], frame[2]]) as usize;
        let (cols, rows) =
            parse_shell_resize_payload(&frame[3..3 + len]).expect("parse resize payload");
        assert_eq!(cols, 200);
        assert_eq!(rows, 50);
    }

    #[test]
    fn shell_exit_roundtrip() {
        let payload = build_shell_exit_payload(42);
        let code = parse_shell_exit_payload(&payload).expect("parse exit");
        assert_eq!(code, 42);
    }

    #[test]
    fn shell_frame_overflow_rejected() {
        let huge = vec![0u8; (u16::MAX as usize) + 1];
        assert!(build_shell_frame(SHELL_MSG_OUTPUT, &huge).is_err());
    }

    #[test]
    fn file_transfer_frame_roundtrip() {
        let payload = b"{\"hello\":\"world\"}";
        let frame =
            build_file_transfer_frame(FILE_TRANSFER_MSG_JSON, payload).expect("build frame");
        let parsed = parse_file_transfer_frame(&frame).expect("parse frame");
        assert_eq!(parsed.message_type, FILE_TRANSFER_MSG_JSON);
        assert_eq!(parsed.payload, payload);
    }

    #[test]
    fn file_transfer_binary_frame_roundtrip() {
        let payload = vec![0u8, 1, 2, 3, 254, 255];
        let frame = build_file_transfer_frame(FILE_TRANSFER_MSG_DATA, &payload)
            .expect("build binary frame");
        let parsed = parse_file_transfer_frame(&frame).expect("parse binary frame");
        assert_eq!(parsed.message_type, FILE_TRANSFER_MSG_DATA);
        assert_eq!(parsed.payload, payload.as_slice());
    }

    #[test]
    fn file_transfer_request_serde() {
        let req = FileTransferRequest::Upload {
            transfer_id: "transfer-1".to_string(),
            destination_path: "C:\\Temp".to_string(),
            file_name: "sample.zip".to_string(),
            is_archive: true,
            extract_archive: true,
            conflict_mode: FileTransferConflictMode::Overwrite,
            expected_size_bytes: 42,
            resume_offset: 0,
        };
        let encoded = serde_json::to_string(&req).expect("serialize upload request");
        let decoded: FileTransferRequest =
            serde_json::from_str(&encoded).expect("deserialize upload request");
        assert_eq!(decoded, req);
    }

    #[test]
    fn file_transfer_response_serde() {
        let resp = FileTransferResponse::TransferComplete {
            transfer_id: "transfer-2".to_string(),
            bytes_transferred: 1024,
            extracted_entries: 7,
        };
        let encoded = serde_json::to_string(&resp).expect("serialize transfer complete");
        let decoded: FileTransferResponse =
            serde_json::from_str(&encoded).expect("deserialize transfer complete");
        assert_eq!(decoded, resp);
    }

    #[test]
    fn display_move_rect_roundtrip() {
        let record = build_display_move_rect(7, 10, 11, 20, 21, 320, 64);
        let decoded = decode_display_record(&record).expect("decode move rect");
        assert_eq!(
            decoded,
            DisplayRecord::MoveRect {
                frame_id: 7,
                src_x: 10,
                src_y: 11,
                dst_x: 20,
                dst_y: 21,
                width: 320,
                height: 64,
            }
        );
    }

    #[test]
    fn display_move_rect_invalid_payload_rejected() {
        let mut invalid = vec![super::DISPLAY_RECORD_MOVE_RECT];
        invalid.extend_from_slice(&31u32.to_le_bytes());
        invalid.resize(5 + 31, 0);
        let decoded = decode_display_record(&invalid).expect_err("reject invalid move rect");
        assert_eq!(decoded, DisplayRecordParseError::InvalidPayload);
    }

    #[test]
    fn display_atlas_h264_roundtrip() {
        let rects = vec![
            DisplayAtlasRect {
                dst_x: 10,
                dst_y: 20,
                width: 32,
                height: 16,
                atlas_x: 0,
                atlas_y: 0,
            },
            DisplayAtlasRect {
                dst_x: 64,
                dst_y: 96,
                width: 8,
                height: 8,
                atlas_x: 32,
                atlas_y: 0,
            },
        ];
        let payload = vec![0, 0, 0, 1, 0x67, 0x64, 0x00, 0x1f];
        let record = build_display_atlas_h264(
            9,
            DISPLAY_ATLAS_H264_FLAG_KEYFRAME,
            640,
            480,
            &rects,
            &payload,
        );
        let decoded = decode_display_record(&record).expect("decode atlas h264");
        assert_eq!(
            decoded,
            DisplayRecord::AtlasH264 {
                frame_id: 9,
                flags: DISPLAY_ATLAS_H264_FLAG_KEYFRAME,
                atlas_width: 640,
                atlas_height: 480,
                rects,
                payload,
            }
        );
    }

    #[test]
    fn display_atlas_h264_invalid_payload_rejected() {
        let mut invalid = vec![super::DISPLAY_RECORD_ATLAS_H264];
        invalid.extend_from_slice(&30u32.to_le_bytes());
        invalid.resize(5 + 30, 0);
        let decoded = decode_display_record(&invalid).expect_err("reject invalid atlas h264");
        assert_eq!(decoded, DisplayRecordParseError::InvalidPayload);
    }

    #[test]
    fn display_experimental_atlas_commands_roundtrip() {
        let rects = vec![DisplayAtlasRect {
            dst_x: 10,
            dst_y: 20,
            width: 2,
            height: 1,
            atlas_x: 0,
            atlas_y: 0,
        }];
        let tile_commands = vec![0x52, 0x4d, 0x54, 0x43, 1, 0, 0, 0];
        let record = build_display_experimental_atlas_commands(11, 2, 1, &rects, &tile_commands);
        let decoded = decode_display_record(&record).expect("decode experimental atlas commands");
        assert_eq!(
            decoded,
            DisplayRecord::ExperimentalAtlasCommands {
                frame_id: 11,
                atlas_width: 2,
                atlas_height: 1,
                rects,
                tile_commands,
            }
        );
    }

    #[test]
    fn display_experimental_atlas_commands_invalid_len_rejected() {
        let rects = vec![DisplayAtlasRect {
            dst_x: 0,
            dst_y: 0,
            width: 1,
            height: 1,
            atlas_x: 0,
            atlas_y: 0,
        }];
        let mut record = build_display_experimental_atlas_commands(12, 1, 1, &rects, &[3]);
        record.pop();
        let decoded =
            decode_display_record(&record).expect_err("reject invalid experimental atlas commands");
        assert_eq!(decoded, DisplayRecordParseError::LengthMismatch);
    }

    #[test]
    fn display_experimental_atlas_commands_allows_move_only_marker() {
        let record = build_display_experimental_atlas_commands(13, 1, 1, &[], &[]);
        let decoded = decode_display_record(&record).expect("decode move-only experimental marker");
        assert_eq!(
            decoded,
            DisplayRecord::ExperimentalAtlasCommands {
                frame_id: 13,
                atlas_width: 1,
                atlas_height: 1,
                rects: Vec::new(),
                tile_commands: Vec::new(),
            }
        );
    }

    #[test]
    fn display_experimental_atlas_command_chunk_roundtrip() {
        let rects = vec![DisplayAtlasRect {
            dst_x: 0,
            dst_y: 32,
            width: 64,
            height: 32,
            atlas_x: 0,
            atlas_y: 32,
        }];
        let tile_commands = vec![0x41, 0x54, 0x58, 0x32, 2, 0, 0, 0];
        let record = build_display_experimental_atlas_commands_chunk(
            14,
            DISPLAY_EXPERIMENTAL_ATLAS_CHUNK_FLAG_PROGRESSIVE
                | DISPLAY_EXPERIMENTAL_ATLAS_CHUNK_FLAG_FINAL,
            2,
            3,
            128,
            96,
            &rects,
            &tile_commands,
        );
        let decoded =
            decode_display_record(&record).expect("decode experimental atlas command chunk");
        assert_eq!(
            decoded,
            DisplayRecord::ExperimentalAtlasCommandsChunk {
                frame_id: 14,
                flags: DISPLAY_EXPERIMENTAL_ATLAS_CHUNK_FLAG_PROGRESSIVE
                    | DISPLAY_EXPERIMENTAL_ATLAS_CHUNK_FLAG_FINAL,
                chunk_index: 2,
                chunk_count: 3,
                atlas_width: 128,
                atlas_height: 96,
                rects,
                tile_commands,
            }
        );
    }

    #[test]
    fn registry_request_serde_uses_camelcase_fields() {
        let req = RegistryRequest::ListKeys {
            request_id: "req-1".to_string(),
            session_id: "session-1".to_string(),
            hive: RegistryHive::HKLM,
            path: "SOFTWARE".to_string(),
            offset: 0,
            limit: 128,
        };
        let encoded = serde_json::to_string(&req).expect("serialize registry request");
        assert!(
            encoded.contains("\"requestId\""),
            "expected requestId in json: {encoded}"
        );
        assert!(
            !encoded.contains("request_id"),
            "unexpected request_id in json: {encoded}"
        );
    }

    #[test]
    fn registry_response_envelope_serde_uses_type_and_request_id() {
        let env = RegistryResponseEnvelope {
            message_type: REGISTRY_META_MESSAGE_TYPE.to_string(),
            request_id: "req-2".to_string(),
            session_id: "session-1".to_string(),
            response: RegistryResponse::Ok {},
        };
        let encoded = serde_json::to_string(&env).expect("serialize registry response envelope");
        assert!(
            encoded.contains("\"type\""),
            "expected type field: {encoded}"
        );
        assert!(
            encoded.contains("\"requestId\""),
            "expected requestId in json: {encoded}"
        );
        let decoded: RegistryResponseEnvelope =
            serde_json::from_str(&encoded).expect("deserialize registry response envelope");
        assert_eq!(decoded, env);
    }

    #[test]
    fn registry_value_entry_serde_uses_camelcase_value_type() {
        let entry = RegistryValueEntry {
            name: "Demo".to_string(),
            value_type: "REG_DWORD".to_string(),
            raw_type: 4,
            data: RegistryValueData::Dword { data: 1 },
        };
        let encoded = serde_json::to_string(&entry).expect("serialize registry value entry");
        assert!(
            encoded.contains("\"valueType\""),
            "expected valueType in json: {encoded}"
        );
        assert!(
            encoded.contains("\"rawType\""),
            "expected rawType in json: {encoded}"
        );
        let decoded: RegistryValueEntry =
            serde_json::from_str(&encoded).expect("deserialize registry value entry");
        assert_eq!(decoded, entry);
    }

    #[test]
    fn chat_frame_roundtrip() {
        let payload = br#"{"kind":"message","id":"a","fromViewer":true,"text":"hi"}"#;
        let frame = build_chat_frame(CHAT_MSG_TEXT, payload).expect("build chat frame");
        let (t, body) = parse_chat_frame(&frame).expect("parse chat frame");
        assert_eq!(t, CHAT_MSG_TEXT);
        assert_eq!(body, payload.as_slice());
    }

    #[test]
    fn chat_auth_frame() {
        let token = b"tok";
        let frame = build_chat_frame(CHAT_MSG_AUTH, token).expect("build chat auth");
        let (t, body) = parse_chat_frame(&frame).expect("parse");
        assert_eq!(t, CHAT_MSG_AUTH);
        assert_eq!(body, token);
    }

    #[test]
    fn chat_wire_payload_serde() {
        let p = ChatWirePayload::Message {
            id: "1".to_string(),
            from_viewer: false,
            text: "hello".to_string(),
            ts_unix_ms: Some(123),
        };
        let j = serde_json::to_string(&p).unwrap();
        let q: ChatWirePayload = serde_json::from_str(&j).unwrap();
        assert_eq!(p, q);
    }

    #[test]
    fn chat_ack_frame_roundtrip() {
        let payload = ChatAckPayload {
            message_id: "msg-1".to_string(),
        };
        let body = serde_json::to_vec(&payload).expect("serialize chat ack");
        let frame = build_chat_frame(CHAT_MSG_ACK, &body).expect("build chat ack");
        let (t, parsed_body) = parse_chat_frame(&frame).expect("parse chat ack");
        assert_eq!(t, CHAT_MSG_ACK);
        let parsed: ChatAckPayload =
            serde_json::from_slice(parsed_body).expect("deserialize chat ack");
        assert_eq!(payload, parsed);
    }

    #[test]
    fn worker_chat_control_reboot_notice_roundtrip() {
        let payload = WorkerChatControlPayload::RebootNoticeAction {
            notice_id: "notice-1".to_string(),
            action: RebootNoticeAction::RebootNow,
        };
        let body = serde_json::to_vec(&payload).expect("serialize reboot notice action");
        let frame = build_chat_frame(CHAT_MSG_CONTROL, &body).expect("build control frame");
        let (t, parsed_body) = parse_chat_frame(&frame).expect("parse control frame");

        assert_eq!(t, CHAT_MSG_CONTROL);
        let parsed: WorkerChatControlPayload =
            serde_json::from_slice(parsed_body).expect("deserialize reboot notice action");
        assert_eq!(payload, parsed);
    }

    #[test]
    fn worker_chat_control_ai_runner_approval_roundtrip() {
        let request = WorkerChatControlPayload::AiRunnerApprovalRequest {
            approval_id: "approval-1".to_string(),
            requester_label: "operator@example.com".to_string(),
            requester_email: Some("operator@example.com".to_string()),
            organization_name: Some("Example Org".to_string()),
            device_label: "macbook-1".to_string(),
            reason: "Capture the current screen".to_string(),
            expires_at_unix_ms: 1_766_000_000_000,
            approval_window_expires_at_unix_ms: 1_766_000_900_000,
        };
        let body = serde_json::to_vec(&request).expect("serialize approval request");
        let frame = build_chat_frame(CHAT_MSG_CONTROL, &body).expect("build approval frame");
        let (t, parsed_body) = parse_chat_frame(&frame).expect("parse approval frame");

        assert_eq!(t, CHAT_MSG_CONTROL);
        let parsed: WorkerChatControlPayload =
            serde_json::from_slice(parsed_body).expect("deserialize approval request");
        assert_eq!(request, parsed);

        let decision = WorkerChatControlPayload::AiRunnerApprovalDecision {
            approval_id: "approval-1".to_string(),
            approved: true,
        };
        let body = serde_json::to_vec(&decision).expect("serialize approval decision");
        let parsed: WorkerChatControlPayload =
            serde_json::from_slice(&body).expect("deserialize approval decision");
        assert_eq!(decision, parsed);
    }

    #[test]
    fn chat_error_payload_no_interactive_user_roundtrip() {
        let payload = ChatWireErrorPayload {
            code: OperationErrorCode::NoInteractiveUser,
            message: "No interactive user is currently logged in.".to_string(),
            retryable: true,
        };
        let body = serde_json::to_vec(&payload).expect("serialize chat error");
        assert!(String::from_utf8_lossy(&body).contains("no_interactive_user"));

        let frame = build_chat_frame(CHAT_MSG_ERROR, &body).expect("build error frame");
        let (t, parsed_body) = parse_chat_frame(&frame).expect("parse error frame");

        assert_eq!(t, CHAT_MSG_ERROR);
        let parsed: ChatWireErrorPayload =
            serde_json::from_slice(parsed_body).expect("deserialize chat error");
        assert_eq!(payload, parsed);
    }

    #[test]
    fn chat_frame_overflow_rejected() {
        let huge = vec![0u8; (u16::MAX as usize) + 1];
        assert!(build_chat_frame(CHAT_MSG_TEXT, &huge).is_err());
    }
}
