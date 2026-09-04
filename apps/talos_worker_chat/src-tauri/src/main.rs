#![cfg_attr(windows, windows_subsystem = "windows")]

use std::fs::{self, OpenOptions};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde_json::{json, Value};
use talos_log_util::DailyFileMakeWriter;
use talos_protocol::{
    build_chat_frame, ChatAckPayload, ChatWirePayload, RebootNoticeAction,
    WorkerChatControlPayload, CHAT_MSG_ACK, CHAT_MSG_AUTH, CHAT_MSG_CONTROL, CHAT_MSG_ERROR,
    CHAT_MSG_TEXT,
};
use tauri::{AppHandle, Emitter, Manager, State, UserAttentionType};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::tcp::OwnedWriteHalf;
use tokio::net::TcpStream;
use tokio::sync::Mutex as AsyncMutex;
use tracing::{debug, error, info, trace, warn};

static AGENT_CHAT_LOG_PATH: OnceLock<PathBuf> = OnceLock::new();
const CHAT_LOG_TARGET: &str = "rmm_chat";
const MAX_CHAT_HISTORY: usize = 256;

#[derive(Clone)]
struct ChatState(Arc<AsyncMutex<ChatStateInner>>);

#[derive(Clone)]
struct LaunchState {
    mode: String,
}

struct ChatStateInner {
    write: Option<OwnedWriteHalf>,
    connected: bool,
    messages: Vec<Value>,
    ai_approval_request: Option<Value>,
    ai_approval_action_sent: bool,
}

#[derive(Clone)]
struct RebootNoticeState(Arc<AsyncMutex<RebootNoticeStateInner>>);

struct RebootNoticeStateInner {
    write: Option<OwnedWriteHalf>,
    connected: bool,
    notice_id: String,
    deadline_unix_ms: u64,
    deferrals_used: u32,
    max_deferrals: u32,
    delay_minutes: u32,
    action_sent: bool,
}

impl ChatState {
    fn new() -> Self {
        Self(Arc::new(AsyncMutex::new(ChatStateInner {
            write: None,
            connected: false,
            messages: Vec::new(),
            ai_approval_request: None,
            ai_approval_action_sent: false,
        })))
    }
}

impl RebootNoticeState {
    fn new(
        notice_id: String,
        deadline_unix_ms: u64,
        deferrals_used: u32,
        max_deferrals: u32,
        delay_minutes: u32,
    ) -> Self {
        Self(Arc::new(AsyncMutex::new(RebootNoticeStateInner {
            write: None,
            connected: false,
            notice_id,
            deadline_unix_ms,
            deferrals_used,
            max_deferrals,
            delay_minutes,
            action_sent: false,
        })))
    }
}

fn cli_value(flag: &str) -> Option<String> {
    let prefix = format!("{flag}=");
    std::env::args()
        .find(|a| a.starts_with(&prefix))
        .and_then(|a| a.strip_prefix(&prefix).map(|s| s.to_string()))
}

fn cli_u64(flag: &str, fallback: u64) -> u64 {
    cli_value(flag)
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(fallback)
}

fn cli_u32(flag: &str, fallback: u32) -> u32 {
    cli_value(flag)
        .and_then(|value| value.parse::<u32>().ok())
        .unwrap_or(fallback)
}

fn cli_optional_value(flag: &str) -> Option<String> {
    cli_value(flag).and_then(|value| {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    })
}

#[cfg(target_os = "windows")]
fn log_path_candidates() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Ok(base) = std::env::var("PROGRAMDATA") {
        paths.push(
            PathBuf::from(base)
                .join("Talos")
                .join("logs")
                .join("talos_worker_chat.log"),
        );
    }
    paths.push(PathBuf::from(
        r"C:\ProgramData\Talos\logs\talos_worker_chat.log",
    ));
    paths.push(std::env::temp_dir().join("talos_worker_chat.log"));
    paths.push(PathBuf::from(r"C:\Windows\Temp\talos_worker_chat.log"));
    paths
}

#[cfg(target_os = "macos")]
fn log_path_candidates() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Ok(home) = std::env::var("HOME") {
        let home = home.trim();
        if !home.is_empty() && home != "/var/root" {
            paths.push(
                PathBuf::from(home)
                    .join("Library")
                    .join("Logs")
                    .join("Talos")
                    .join("talos_worker_chat.log"),
            );
        }
    }
    paths.push(PathBuf::from("/Library/Logs/Talos/talos_worker_chat.log"));
    paths.push(std::env::temp_dir().join("talos_worker_chat.log"));
    paths
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn log_path_candidates() -> Vec<PathBuf> {
    vec![std::env::temp_dir().join("talos_worker_chat.log")]
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
        .unwrap_or_else(|| std::env::temp_dir().join("talos_worker_chat.log"))
}

fn agent_chat_log_path() -> PathBuf {
    AGENT_CHAT_LOG_PATH.get_or_init(resolve_log_path).clone()
}

fn agent_chat_log_filter() -> tracing_subscriber::EnvFilter {
    tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info,rmm_chat=trace"))
}

fn init_file_logging() -> Result<(), std::io::Error> {
    let log_path = agent_chat_log_path();
    if let Some(parent) = log_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let writer = DailyFileMakeWriter::try_new(log_path.clone())?;
    tracing_subscriber::fmt()
        .with_env_filter(agent_chat_log_filter())
        .with_timer(tracing_subscriber::fmt::time::SystemTime)
        .with_writer(writer)
        .with_ansi(false)
        .init();
    info!(path = %log_path.display(), "Talos Worker chat logging to file");
    Ok(())
}

#[cfg(target_os = "windows")]
fn init_webview_user_data_dir() {
    if std::env::var("WEBVIEW2_USER_DATA_FOLDER")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .is_some()
    {
        return;
    }

    let dir = std::env::var("LOCALAPPDATA")
        .map(|base| {
            PathBuf::from(base)
                .join("Talos")
                .join("TalosWorkerChat")
                .join("WebView2")
        })
        .or_else(|_| {
            std::env::var("PROGRAMDATA").map(|base| {
                PathBuf::from(base)
                    .join("Talos")
                    .join("TalosWorkerChat")
                    .join("WebView2")
            })
        })
        .unwrap_or_else(|_| {
            std::env::temp_dir()
                .join("Talos")
                .join("TalosWorkerChat")
                .join("WebView2")
        });
    if let Err(err) = fs::create_dir_all(&dir) {
        warn!(
            path = %dir.display(),
            error = %err,
            "failed to create fallback WebView2 user data dir"
        );
        return;
    }
    std::env::set_var("WEBVIEW2_USER_DATA_FOLDER", &dir);
    info!(
        path = %dir.display(),
        "set fallback WebView2 user data dir"
    );
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn chat_message_event(
    id: String,
    from_viewer: bool,
    text: String,
    ts_unix_ms: Option<u64>,
) -> serde_json::Value {
    json!({
        "kind": "message",
        "id": id,
        "fromViewer": from_viewer,
        "from_viewer": from_viewer,
        "text": text,
        "tsUnixMs": ts_unix_ms,
        "ts_unix_ms": ts_unix_ms,
    })
}

fn chat_ack_event(message_id: String) -> serde_json::Value {
    json!({
        "messageId": message_id,
        "message_id": message_id,
    })
}

fn ai_approval_request_event(
    approval_id: String,
    requester_label: String,
    requester_email: Option<String>,
    organization_name: Option<String>,
    device_label: String,
    reason: String,
    expires_at_unix_ms: u64,
    approval_window_expires_at_unix_ms: u64,
    action_sent: bool,
) -> serde_json::Value {
    json!({
        "kind": "ai_runner_approval_request",
        "approvalId": approval_id,
        "approval_id": approval_id,
        "requesterLabel": requester_label,
        "requester_label": requester_label,
        "requesterEmail": requester_email,
        "requester_email": requester_email,
        "organizationName": organization_name,
        "organization_name": organization_name,
        "deviceLabel": device_label,
        "device_label": device_label,
        "reason": reason,
        "expiresAtUnixMs": expires_at_unix_ms,
        "expires_at_unix_ms": expires_at_unix_ms,
        "approvalWindowExpiresAtUnixMs": approval_window_expires_at_unix_ms,
        "approval_window_expires_at_unix_ms": approval_window_expires_at_unix_ms,
        "actionSent": action_sent,
        "action_sent": action_sent,
    })
}

fn trim_chat_history(messages: &mut Vec<Value>) {
    if messages.len() > MAX_CHAT_HISTORY {
        let drop_count = messages.len() - MAX_CHAT_HISTORY;
        messages.drain(0..drop_count);
    }
}

async fn read_chat_tcp_frame<R: tokio::io::AsyncRead + Unpin>(
    read: &mut R,
) -> Result<Option<(u8, Vec<u8>)>> {
    let mut hdr = [0u8; 3];
    if let Err(err) = AsyncReadExt::read_exact(read, &mut hdr).await {
        let message = err.to_string();
        if message.contains("unexpected end") || message.contains("early eof") {
            return Ok(None);
        }
        return Err(anyhow::anyhow!("tcp chat header: {message}"));
    }
    let len = u16::from_be_bytes([hdr[1], hdr[2]]) as usize;
    if len > talos_protocol::CHAT_MAX_PAYLOAD_LEN {
        return Err(anyhow::anyhow!("chat tcp payload too large"));
    }
    let mut payload = vec![0u8; len];
    if len > 0 {
        AsyncReadExt::read_exact(read, &mut payload)
            .await
            .context("tcp chat payload")?;
    }
    Ok(Some((hdr[0], payload)))
}

async fn bridge_loop(app: AppHandle, state: ChatState, port: u16, secret: String) -> Result<()> {
    debug!(
        target: CHAT_LOG_TARGET,
        port,
        "agent chat connecting local bridge"
    );
    let mut stream = TcpStream::connect(("127.0.0.1", port))
        .await
        .with_context(|| format!("connect chat bridge 127.0.0.1:{port}"))?;

    let auth = build_chat_frame(CHAT_MSG_AUTH, secret.as_bytes()).context("build auth frame")?;
    stream.write_all(&auth).await.context("write auth")?;
    debug!(
        target: CHAT_LOG_TARGET,
        port,
        "agent chat bridge auth sent"
    );

    let (mut read_half, write_half) = stream.into_split();
    {
        let mut g = state.0.lock().await;
        g.write = Some(write_half);
        g.connected = true;
    }
    if let Err(err) = app.emit("chat/status", json!({ "connected": true })) {
        warn!(target: CHAT_LOG_TARGET, error = %err, "agent chat connected status emit failed");
    } else {
        trace!(target: CHAT_LOG_TARGET, "agent chat connected status emitted");
    }
    info!(
        target: CHAT_LOG_TARGET,
        port,
        "agent chat bridge connected"
    );

    loop {
        let Some((ty, body)) = read_chat_tcp_frame(&mut read_half).await? else {
            {
                let mut g = state.0.lock().await;
                g.connected = false;
                g.write = None;
            }
            if let Err(err) = app.emit("chat/status", json!({ "connected": false })) {
                warn!(target: CHAT_LOG_TARGET, error = %err, "agent chat disconnected status emit failed");
            }
            warn!(
                target: CHAT_LOG_TARGET,
                port,
                "agent chat bridge closed by service"
            );
            break;
        };
        trace!(
            target: CHAT_LOG_TARGET,
            frame_type = ty,
            body_len = body.len(),
            "agent chat frame received from service"
        );

        if ty == CHAT_MSG_TEXT {
            match serde_json::from_slice::<ChatWirePayload>(&body) {
                Ok(ChatWirePayload::Message {
                    id,
                    from_viewer,
                    text,
                    ts_unix_ms,
                }) => {
                    debug!(
                        target: CHAT_LOG_TARGET,
                        message_id = %id,
                        from_viewer,
                        text_len = text.len(),
                        "agent chat inbound message emitting to window"
                    );
                    let event = chat_message_event(id.clone(), from_viewer, text, ts_unix_ms);
                    {
                        let mut g = state.0.lock().await;
                        g.messages.push(event.clone());
                        trim_chat_history(&mut g.messages);
                    }
                    if let Err(err) = app.emit("chat/inbound", event) {
                        warn!(
                            target: CHAT_LOG_TARGET,
                            error = %err,
                            message_id = %id,
                            "agent chat inbound event emit failed"
                        );
                    }
                    trace!(
                        target: CHAT_LOG_TARGET,
                        message_id = %id,
                        "agent chat ack writing to service"
                    );
                    let ack = serde_json::to_vec(&ChatAckPayload { message_id: id })
                        .context("serialize chat ack")?;
                    let ack_frame =
                        build_chat_frame(CHAT_MSG_ACK, &ack).context("build chat ack")?;
                    let mut guard = state.0.lock().await;
                    if let Some(w) = guard.write.as_mut() {
                        w.write_all(&ack_frame).await.context("write chat ack")?;
                    }
                }
                Ok(ChatWirePayload::SidecarReady {}) => {
                    trace!(target: CHAT_LOG_TARGET, "agent chat sidecar-ready frame ignored");
                }
                Err(err) => {
                    warn!(target: CHAT_LOG_TARGET, error = %err, "ignored non-chat JSON frame");
                }
            }
        } else if ty == CHAT_MSG_CONTROL {
            match serde_json::from_slice::<WorkerChatControlPayload>(&body) {
                Ok(WorkerChatControlPayload::AiRunnerApprovalRequest {
                    approval_id,
                    requester_label,
                    requester_email,
                    organization_name,
                    device_label,
                    reason,
                    expires_at_unix_ms,
                    approval_window_expires_at_unix_ms,
                }) => {
                    let event = ai_approval_request_event(
                        approval_id.clone(),
                        requester_label,
                        requester_email,
                        organization_name,
                        device_label,
                        reason,
                        expires_at_unix_ms,
                        approval_window_expires_at_unix_ms,
                        false,
                    );
                    {
                        let mut g = state.0.lock().await;
                        g.ai_approval_request = Some(event.clone());
                        g.ai_approval_action_sent = false;
                    }
                    if let Err(err) = app.emit("approval/request", event.clone()) {
                        warn!(
                            target: CHAT_LOG_TARGET,
                            error = %err,
                            approval_id = %approval_id,
                            "agent chat AI runner approval request emit failed"
                        );
                    } else {
                        debug!(
                            target: CHAT_LOG_TARGET,
                            approval_id = %approval_id,
                            "agent chat AI runner approval request emitted"
                        );
                    }
                    debug!(
                        target: CHAT_LOG_TARGET,
                        approval_id = %approval_id,
                        "agent chat AI runner approval request received"
                    );
                }
                Ok(_) => {
                    trace!(target: CHAT_LOG_TARGET, "agent chat ignored unrelated control frame");
                }
                Err(err) => {
                    warn!(target: CHAT_LOG_TARGET, error = %err, "ignored invalid control frame");
                }
            }
        } else if ty == CHAT_MSG_ACK {
            if let Ok(ack) = serde_json::from_slice::<ChatAckPayload>(&body) {
                trace!(
                    target: CHAT_LOG_TARGET,
                    message_id = %ack.message_id,
                    "agent chat ack received from service"
                );
                let message_id = ack.message_id;
                if let Err(err) = app.emit("chat/ack", chat_ack_event(message_id.clone())) {
                    warn!(
                        target: CHAT_LOG_TARGET,
                        error = %err,
                        message_id = %message_id,
                        "agent chat ack event emit failed"
                    );
                }
            }
        } else if ty == CHAT_MSG_ERROR {
            {
                let mut g = state.0.lock().await;
                g.connected = false;
            }
            if let Err(err) = app.emit(
                "chat/status",
                json!({
                    "connected": false,
                    "error": String::from_utf8_lossy(&body),
                }),
            ) {
                warn!(target: CHAT_LOG_TARGET, error = %err, "agent chat error status emit failed");
            }
            warn!(
                target: CHAT_LOG_TARGET,
                error = %String::from_utf8_lossy(&body),
                "agent chat error frame received"
            );
        }
    }

    let mut g = state.0.lock().await;
    g.write = None;
    g.connected = false;
    Ok(())
}

async fn write_reboot_notice_control_frame(
    write: &mut OwnedWriteHalf,
    payload: WorkerChatControlPayload,
) -> Result<()> {
    let body = serde_json::to_vec(&payload).context("serialize reboot notice control")?;
    let frame = build_chat_frame(CHAT_MSG_CONTROL, &body).context("build reboot notice control")?;
    write
        .write_all(&frame)
        .await
        .context("write reboot notice control")?;
    Ok(())
}

async fn write_ai_runner_approval_decision(
    approval_id: String,
    approved: bool,
    state: &ChatState,
) -> Result<bool, String> {
    let mut guard = state.0.lock().await;
    if guard.ai_approval_action_sent {
        return Ok(false);
    }
    let writer = guard
        .write
        .as_mut()
        .ok_or_else(|| "Chat bridge is not connected".to_string())?;
    let payload = WorkerChatControlPayload::AiRunnerApprovalDecision {
        approval_id: approval_id.clone(),
        approved,
    };
    let body = serde_json::to_vec(&payload).map_err(|error| error.to_string())?;
    let frame = build_chat_frame(CHAT_MSG_CONTROL, &body).map_err(|error| error.to_string())?;
    writer
        .write_all(&frame)
        .await
        .map_err(|error| error.to_string())?;
    guard.ai_approval_action_sent = true;
    if let Some(request) = guard.ai_approval_request.as_mut() {
        if let Some(object) = request.as_object_mut() {
            object.insert("actionSent".to_string(), json!(true));
            object.insert("action_sent".to_string(), json!(true));
        }
    }
    Ok(true)
}

async fn reboot_notice_bridge_loop(
    app: AppHandle,
    state: RebootNoticeState,
    port: u16,
    secret: String,
) -> Result<()> {
    let notice_id = {
        let guard = state.0.lock().await;
        guard.notice_id.clone()
    };
    debug!(
        target: CHAT_LOG_TARGET,
        port,
        notice_id = %notice_id,
        "reboot notice connecting local bridge"
    );
    let mut stream = TcpStream::connect(("127.0.0.1", port))
        .await
        .with_context(|| format!("connect reboot notice bridge 127.0.0.1:{port}"))?;

    let auth = build_chat_frame(CHAT_MSG_AUTH, secret.as_bytes()).context("build auth frame")?;
    stream.write_all(&auth).await.context("write auth")?;
    let (mut read_half, mut write_half) = stream.into_split();
    write_reboot_notice_control_frame(
        &mut write_half,
        WorkerChatControlPayload::RebootNoticeReady {
            notice_id: notice_id.clone(),
        },
    )
    .await?;
    {
        let mut guard = state.0.lock().await;
        guard.write = Some(write_half);
        guard.connected = true;
    }
    let _ = app.emit(
        "reboot/status",
        json!({ "connected": true, "noticeId": notice_id }),
    );
    info!(
        target: CHAT_LOG_TARGET,
        port,
        "reboot notice bridge connected"
    );

    loop {
        let frame = read_chat_tcp_frame(&mut read_half).await;
        match frame {
            Ok(Some(_)) => {}
            Ok(None) => break,
            Err(error) => {
                warn!(target: CHAT_LOG_TARGET, %error, "reboot notice bridge read failed");
                break;
            }
        }
    }

    let mut guard = state.0.lock().await;
    guard.connected = false;
    guard.write = None;
    let _ = app.emit("reboot/status", json!({ "connected": false }));
    Ok(())
}

#[tauri::command]
async fn send_chat_message(
    text: String,
    state: State<'_, ChatState>,
) -> Result<serde_json::Value, String> {
    let id = uuid::Uuid::new_v4().to_string();
    let ts_unix_ms = Some(now_ms());
    let payload = ChatWirePayload::Message {
        id: id.clone(),
        from_viewer: false,
        text: text.clone(),
        ts_unix_ms,
    };
    let body = serde_json::to_vec(&payload).map_err(|e| e.to_string())?;
    let frame = build_chat_frame(CHAT_MSG_TEXT, &body).map_err(|e| e.to_string())?;

    let mut guard = state.0.lock().await;
    let w = guard
        .write
        .as_mut()
        .ok_or_else(|| "Chat bridge is not connected".to_string())?;
    w.write_all(&frame).await.map_err(|e| e.to_string())?;
    let event = chat_message_event(id.clone(), false, text.clone(), ts_unix_ms);
    guard.messages.push(event.clone());
    trim_chat_history(&mut guard.messages);
    debug!(
        target: CHAT_LOG_TARGET,
        message_id = %id,
        text_len = text.len(),
        "agent chat outbound message written to service"
    );
    Ok(event)
}

#[tauri::command]
async fn get_chat_snapshot(state: State<'_, ChatState>) -> Result<serde_json::Value, String> {
    let guard = state.0.lock().await;
    trace!(
        target: CHAT_LOG_TARGET,
        connected = guard.connected,
        message_count = guard.messages.len(),
        has_ai_approval = guard.ai_approval_request.is_some(),
        "agent chat snapshot requested"
    );
    Ok(json!({
        "connected": guard.connected,
        "messages": guard.messages.clone(),
    }))
}

#[tauri::command]
async fn get_app_state(
    launch: State<'_, LaunchState>,
    reboot: State<'_, RebootNoticeState>,
    chat: State<'_, ChatState>,
) -> Result<serde_json::Value, String> {
    let reboot_guard = reboot.0.lock().await;
    let chat_guard = chat.0.lock().await;
    let approval_id = chat_guard
        .ai_approval_request
        .as_ref()
        .and_then(|request| {
            request
                .get("approvalId")
                .or_else(|| request.get("approval_id"))
        })
        .and_then(Value::as_str);
    debug!(
        target: CHAT_LOG_TARGET,
        mode = %launch.mode,
        chat_connected = chat_guard.connected,
        message_count = chat_guard.messages.len(),
        has_ai_approval = chat_guard.ai_approval_request.is_some(),
        approval_id = ?approval_id,
        approval_action_sent = chat_guard.ai_approval_action_sent,
        reboot_connected = reboot_guard.connected,
        "agent chat app state requested"
    );
    Ok(json!({
        "mode": launch.mode.clone(),
        "chatConnected": chat_guard.connected,
        "chat_connected": chat_guard.connected,
        "aiApproval": chat_guard.ai_approval_request.clone(),
        "rebootNotice": {
            "connected": reboot_guard.connected,
            "noticeId": reboot_guard.notice_id.clone(),
            "deadlineUnixMs": reboot_guard.deadline_unix_ms,
            "deferralsUsed": reboot_guard.deferrals_used,
            "maxDeferrals": reboot_guard.max_deferrals,
            "delayMinutes": reboot_guard.delay_minutes,
            "actionSent": reboot_guard.action_sent
        }
    }))
}

#[tauri::command]
async fn log_ui_event(event: String, data: Option<Value>) -> Result<(), String> {
    debug!(
        target: CHAT_LOG_TARGET,
        event = %event,
        data = ?data,
        "agent chat UI event"
    );
    Ok(())
}

#[tauri::command]
async fn send_reboot_notice_action(
    action: String,
    state: State<'_, RebootNoticeState>,
    app: AppHandle,
) -> Result<serde_json::Value, String> {
    let parsed_action = match action.as_str() {
        "defer" => RebootNoticeAction::Defer,
        "reboot_now" => RebootNoticeAction::RebootNow,
        _ => return Err(format!("unsupported reboot notice action: {action}")),
    };

    let mut guard = state.0.lock().await;
    if guard.action_sent {
        return Err("Reboot notice action was already sent".to_string());
    }
    let notice_id = guard.notice_id.clone();
    let writer = guard
        .write
        .as_mut()
        .ok_or_else(|| "Reboot notice bridge is not connected".to_string())?;
    write_reboot_notice_control_frame(
        writer,
        WorkerChatControlPayload::RebootNoticeAction {
            notice_id: notice_id.clone(),
            action: parsed_action,
        },
    )
    .await
    .map_err(|error| error.to_string())?;
    guard.action_sent = true;
    debug!(
        target: CHAT_LOG_TARGET,
        notice_id = %notice_id,
        action = %action,
        "reboot notice action sent"
    );
    let _ = app.emit(
        "reboot/action-sent",
        json!({ "noticeId": notice_id, "action": action }),
    );
    app.exit(0);
    Ok(json!({ "sent": true }))
}

#[tauri::command]
async fn send_ai_runner_approval_decision(
    approval_id: String,
    approved: bool,
    state: State<'_, ChatState>,
    app: AppHandle,
) -> Result<serde_json::Value, String> {
    let sent = write_ai_runner_approval_decision(approval_id.clone(), approved, &state).await?;
    if !sent {
        return Err("Approval decision was already sent".to_string());
    }
    debug!(
        target: CHAT_LOG_TARGET,
        approval_id = %approval_id,
        approved,
        "AI runner approval decision sent"
    );
    let _ = app.emit(
        "approval/status",
        json!({ "approvalId": approval_id, "approval_id": approval_id, "approved": approved, "sent": true }),
    );
    app.exit(0);
    Ok(json!({ "sent": true, "approved": approved }))
}

#[tauri::command]
async fn close_expired_ai_runner_approval(
    approval_id: String,
    state: State<'_, ChatState>,
    app: AppHandle,
) -> Result<serde_json::Value, String> {
    let now = now_ms();
    let expires_at_unix_ms = {
        let guard = state.0.lock().await;
        if guard.ai_approval_action_sent {
            return Ok(json!({ "closed": false, "reason": "action_sent" }));
        }
        let Some(request) = guard.ai_approval_request.as_ref() else {
            return Ok(json!({ "closed": false, "reason": "missing_approval" }));
        };
        let request_approval_id = request
            .get("approvalId")
            .or_else(|| request.get("approval_id"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        if request_approval_id != approval_id {
            return Ok(json!({ "closed": false, "reason": "approval_mismatch" }));
        }
        let expires_at_unix_ms = request
            .get("expiresAtUnixMs")
            .or_else(|| request.get("expires_at_unix_ms"))
            .and_then(Value::as_u64)
            .ok_or_else(|| "Approval request is missing an expiry timestamp".to_string())?;
        if expires_at_unix_ms > now {
            return Ok(json!({
                "closed": false,
                "reason": "not_expired",
                "expiresAtUnixMs": expires_at_unix_ms,
                "nowUnixMs": now,
            }));
        }
        expires_at_unix_ms
    };

    debug!(
        target: CHAT_LOG_TARGET,
        approval_id = %approval_id,
        expires_at_unix_ms,
        now_unix_ms = now,
        "AI runner approval expired; closing worker chat window"
    );
    let _ = app.emit(
        "approval/status",
        json!({
            "approvalId": approval_id,
            "approval_id": approval_id,
            "expired": true,
            "closed": true,
        }),
    );
    app.exit(0);
    Ok(json!({ "closed": true, "reason": "expired" }))
}

fn surface_main_window(app: &tauri::App, compact_notice: bool) {
    let Some(window) = app.get_webview_window("main") else {
        error!("agent chat main window missing during setup");
        return;
    };

    if let Err(err) = window.set_theme(Some(tauri::utils::Theme::Dark)) {
        warn!(error = %err, "agent chat window set dark theme failed");
    }
    if let Err(err) = window.set_background_color(Some(tauri::utils::config::Color(7, 16, 28, 255)))
    {
        warn!(error = %err, "agent chat window set background color failed");
    }
    #[cfg(target_os = "macos")]
    if let Err(err) = window.set_title_bar_style(tauri::TitleBarStyle::Transparent) {
        warn!(error = %err, "agent chat window set title bar style failed");
    }

    if compact_notice {
        if let Err(err) = window.set_title("Talos Update Reboot") {
            warn!(error = %err, "reboot notice window set title failed");
        }
        if let Err(err) =
            window.set_size(tauri::Size::Logical(tauri::LogicalSize::new(460.0, 420.0)))
        {
            warn!(error = %err, "reboot notice window resize failed");
        }
        if let Err(err) = window.set_resizable(false) {
            warn!(error = %err, "reboot notice window set resizable failed");
        }
    }

    let visible_before = window.is_visible().ok();
    let minimized_before = window.is_minimized().ok();
    let focused_before = window.is_focused().ok();
    let outer_position = window.outer_position().ok();
    let outer_size = window.outer_size().ok();
    let monitor = window
        .current_monitor()
        .ok()
        .flatten()
        .map(|m| format!("{}x{}", m.size().width, m.size().height));
    info!(
        visible_before = ?visible_before,
        minimized_before = ?minimized_before,
        focused_before = ?focused_before,
        outer_position = ?outer_position,
        outer_size = ?outer_size,
        monitor = ?monitor,
        "agent chat main window found during setup"
    );

    if let Err(err) = window.unminimize() {
        warn!(error = %err, "agent chat main window unminimize failed");
    }
    if let Err(err) = window.center() {
        warn!(error = %err, "agent chat main window center failed");
    }
    if let Err(err) = window.show() {
        warn!(error = %err, "agent chat main window show failed");
    }
    if let Err(err) = window.set_always_on_top(true) {
        warn!(error = %err, "agent chat main window set always-on-top failed");
    }
    if let Err(err) = window.set_focus() {
        warn!(error = %err, "agent chat main window focus failed");
    }
    if let Err(err) = window.set_always_on_top(false) {
        warn!(error = %err, "agent chat main window clear always-on-top failed");
    }
    if let Err(err) = window.request_user_attention(Some(UserAttentionType::Informational)) {
        warn!(error = %err, "agent chat request user attention failed");
    }

    info!(
        visible_after = ?window.is_visible().ok(),
        minimized_after = ?window.is_minimized().ok(),
        focused_after = ?window.is_focused().ok(),
        "agent chat main window surfaced"
    );
}

fn main() {
    if let Err(err) = init_file_logging() {
        eprintln!("Talos Worker chat file logging init failed: {err}");
        tracing_subscriber::fmt()
            .with_env_filter(agent_chat_log_filter())
            .init();
    }
    #[cfg(target_os = "windows")]
    init_webview_user_data_dir();

    let port: u16 = cli_value("--local-port")
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let secret = cli_value("--bridge-secret").unwrap_or_default();
    let mode = cli_value("--mode").unwrap_or_else(|| "remote-chat".to_string());
    let is_update_reboot = mode == "update-reboot";
    let is_ai_approval = mode == "ai-approval";
    let notice_id = cli_value("--notice-id").unwrap_or_default();
    let deadline_unix_ms = cli_u64("--deadline-unix-ms", now_ms() + 15 * 60 * 1000);
    let deferrals_used = cli_u32("--deferrals-used", 0);
    let max_deferrals = cli_u32("--max-deferrals", 4);
    let delay_minutes = cli_u32("--delay-minutes", 15);
    let approval_id = cli_value("--approval-id").unwrap_or_default();
    let requester_label =
        cli_value("--requester-label").unwrap_or_else(|| "A Talos operator".to_string());
    let requester_email = cli_optional_value("--requester-email");
    let organization_name = cli_optional_value("--organization-name");
    let device_label = cli_value("--device-label").unwrap_or_else(|| "this device".to_string());
    let reason = cli_value("--reason").unwrap_or_else(|| "View the current screen".to_string());
    let approval_expires_at_unix_ms = cli_u64("--expires-at-unix-ms", now_ms() + 5 * 60 * 1000);
    let approval_window_expires_at_unix_ms = cli_u64(
        "--approval-window-expires-at-unix-ms",
        now_ms() + 15 * 60 * 1000,
    );
    info!(
        pid = std::process::id(),
        port,
        has_bridge_secret = !secret.is_empty(),
        mode = %mode,
        "Talos Worker chat process starting"
    );

    let chat_state = ChatState::new();
    if is_ai_approval && !approval_id.trim().is_empty() {
        let event = ai_approval_request_event(
            approval_id.clone(),
            requester_label,
            requester_email,
            organization_name,
            device_label,
            reason,
            approval_expires_at_unix_ms,
            approval_window_expires_at_unix_ms,
            false,
        );
        if let Ok(mut guard) = chat_state.0.try_lock() {
            guard.ai_approval_request = Some(event);
            guard.ai_approval_action_sent = false;
        }
    }
    let reboot_notice_state = RebootNoticeState::new(
        notice_id,
        deadline_unix_ms,
        deferrals_used,
        max_deferrals,
        delay_minutes,
    );
    let launch_state = LaunchState { mode: mode.clone() };

    tauri::Builder::default()
        .manage(chat_state.clone())
        .manage(reboot_notice_state.clone())
        .manage(launch_state)
        .invoke_handler(tauri::generate_handler![
            send_chat_message,
            get_chat_snapshot,
            get_app_state,
            log_ui_event,
            send_reboot_notice_action,
            send_ai_runner_approval_decision,
            close_expired_ai_runner_approval
        ])
        .setup(move |app| {
            let handle = app.handle().clone();
            let chat = chat_state.clone();
            let reboot = reboot_notice_state.clone();
            info!("Talos Worker chat tauri setup started");
            surface_main_window(app, is_update_reboot);

            if port == 0 || secret.is_empty() {
                error!("Talos Worker chat requires --local-port and --bridge-secret");
                let event = if is_update_reboot {
                    "reboot/status"
                } else {
                    "chat/status"
                };
                let _ = handle.emit(
                    event,
                    json!({
                        "connected": false,
                        "error": "missing --local-port or --bridge-secret",
                    }),
                );
                return Ok(());
            }

            if is_update_reboot {
                let _ = handle.emit("reboot/status", json!({ "connected": false }));
                debug!(target: CHAT_LOG_TARGET, "reboot notice initial disconnected status emitted");
                tauri::async_runtime::spawn(async move {
                    if let Err(err) =
                        reboot_notice_bridge_loop(handle.clone(), reboot.clone(), port, secret).await
                    {
                        {
                            let mut g = reboot.0.lock().await;
                            g.connected = false;
                            g.write = None;
                        }
                        error!(target: CHAT_LOG_TARGET, error = %err, "reboot notice bridge failed");
                        let _ = handle.emit(
                            "reboot/status",
                            json!({
                                "connected": false,
                                "error": err.to_string(),
                            }),
                        );
                    }
                });
                return Ok(());
            }

            let _ = handle.emit("chat/status", json!({ "connected": false }));
            debug!(target: CHAT_LOG_TARGET, "agent chat initial disconnected status emitted");

            tauri::async_runtime::spawn(async move {
                if let Err(err) = bridge_loop(handle.clone(), chat.clone(), port, secret).await {
                    {
                        let mut g = chat.0.lock().await;
                        g.connected = false;
                        g.write = None;
                    }
                    error!(target: CHAT_LOG_TARGET, error = %err, "chat bridge failed");
                    let _ = handle.emit(
                        "chat/status",
                        json!({
                            "connected": false,
                            "error": err.to_string(),
                        }),
                    );
                }
            });
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running talos_worker_chat");
}
