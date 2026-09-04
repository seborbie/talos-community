use std::{
    collections::{HashMap, HashSet},
    ffi::CString,
    fs,
    net::UdpSocket,
    os::unix::{ffi::OsStrExt, fs::PermissionsExt},
    path::{Path, PathBuf},
    sync::{atomic::Ordering, Arc},
    time::{Duration, Instant},
};

use anyhow::{anyhow, Context, Result};
use base64::Engine as _;
use chacha20poly1305::ChaCha20Poly1305;
use quinn::{Connection, Endpoint};
use rustls::pki_types::ServerName;
use serde::Deserialize;
use serde_json::json;
use talos_protocol::{
    build_control_frame, parse_control_frame,
    relay_transport::{
        build_e2e_cipher, build_relay_client_tls_config, parse_relay_target, read_e2e_frame_from,
        read_http_response, write_e2e_frame,
    },
    LocalAddr, CONTROL_PAYLOAD_TIMESTAMP_LEN, CONTROL_TYPE_CONNECTION_PING,
    CONTROL_TYPE_REGISTRY_REQUEST, CONTROL_TYPE_SECURE_ATTENTION, CONTROL_TYPE_SESSION_LOGOFF,
    CONTROL_TYPE_SESSION_SWITCH, CONTROL_TYPE_STOP_CAPTURE, DISPLAY_RECORD_FRAME_BEGIN,
    DISPLAY_RECORD_FRAME_END, DISPLAY_RECORD_KEYFRAME, HEARTBEAT_PAYLOAD,
    HELPER_PIPE_HANDSHAKE_MAGIC, HELPER_PIPE_MAX_AUTH_TOKEN_LEN, HELPER_PIPE_PROTOCOL_VERSION,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt},
    net::{TcpStream, UnixListener},
    process::Command,
    sync::RwLock,
    time::{sleep, timeout},
};
use tokio_rustls::TlsConnector;
use tracing::{debug, info, warn};

use crate::{
    find_buffered_chunk, is_relay_connection_closed, send_quic_chunk, send_relay_chunk,
    CapturePipeline, ControlPipeWriter, MacosDesktopCaptureMode,
};

const HELPER_FPS: u32 = 30;
const HEARTBEAT_INTERVAL_SECS: u64 = 15;
const HEARTBEAT_MISSED_THRESHOLD: u32 = 3;
const SOCKET_ACCEPT_TIMEOUT: Duration = Duration::from_secs(20);
const HELPER_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);
const HELPER_CHUNK_READ_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_HELPER_CHUNK_LEN: usize = 64 * 1024 * 1024;
const MACOS_DESKTOP_SOCKET_DIR: &str = "/tmp/trd";
const MACOS_UNIX_SOCKET_MAX_PATH_BYTES: usize = 103;
const CAPTURE_STARTUP_FAILURE_WAIT: Duration = Duration::from_secs(22);
const PERMISSIONS_HELPER_APP_PATH: &str = "/Applications/Talos Permissions Helper.app";
const WORKER_HELPER_APP_PATH: &str = "/Library/Talos/Worker/Talos Worker Helper.app";
const WORKER_HELPER_APP_EXECUTABLE: &str =
    "/Library/Talos/Worker/Talos Worker Helper.app/Contents/MacOS/talos_worker_helper";

#[derive(Deserialize)]
struct HelperCaptureError {
    reason: Option<String>,
    message: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct HelperPermissionSnapshot {
    pub(crate) accessibility: bool,
    #[serde(rename = "screenRecording")]
    pub(crate) screen_recording: bool,
}

pub async fn ensure_capture_pipeline(
    session_id: String,
    capture_mode: MacosDesktopCaptureMode,
    capture_pipelines: &Arc<RwLock<HashMap<String, Arc<CapturePipeline>>>>,
    control_pipe_writers: Arc<RwLock<HashMap<String, ControlPipeWriter>>>,
) -> Arc<CapturePipeline> {
    if let Some(pipeline) = capture_pipelines.read().await.get(&session_id).cloned() {
        return pipeline;
    }
    let mut write_guard = capture_pipelines.write().await;
    if let Some(pipeline) = write_guard.get(&session_id).cloned() {
        return pipeline;
    }

    let pipeline = Arc::new(CapturePipeline::new());
    write_guard.insert(session_id.clone(), pipeline.clone());
    drop(write_guard);

    let auth_token = uuid::Uuid::new_v4().simple().to_string();
    let uid = match active_console_uid()
        .await
        .and_then(validate_remote_desktop_console_uid)
    {
        Ok(uid) => uid,
        Err(err) => {
            warn!(session_id = %session_id, error = %err, "macOS remote desktop helper launch skipped; active console user unavailable");
            remove_failed_pipeline(&session_id, &pipeline, capture_pipelines).await;
            return pipeline;
        }
    };
    let sockets = match prepare_socket_paths(&session_id) {
        Ok(paths) => paths,
        Err(err) => {
            warn!(session_id = %session_id, error = %err, "macOS remote desktop socket setup failed");
            remove_failed_pipeline(&session_id, &pipeline, capture_pipelines).await;
            return pipeline;
        }
    };

    let stream_listener = match UnixListener::bind(&sockets.stream) {
        Ok(listener) => listener,
        Err(err) => {
            warn!(session_id = %session_id, path = %sockets.stream.display(), error = %err, "macOS stream socket bind failed");
            cleanup_socket_paths(&sockets);
            remove_failed_pipeline(&session_id, &pipeline, capture_pipelines).await;
            return pipeline;
        }
    };
    let control_listener = match UnixListener::bind(&sockets.control) {
        Ok(listener) => listener,
        Err(err) => {
            warn!(session_id = %session_id, path = %sockets.control.display(), error = %err, "macOS control socket bind failed");
            cleanup_socket_paths(&sockets);
            remove_failed_pipeline(&session_id, &pipeline, capture_pipelines).await;
            return pipeline;
        }
    };
    if let Err(err) = apply_socket_permissions(&sockets, uid) {
        warn!(session_id = %session_id, error = %err, "macOS remote desktop socket permission setup failed");
        cleanup_socket_paths(&sockets);
        remove_failed_pipeline(&session_id, &pipeline, capture_pipelines).await;
        return pipeline;
    }

    if let Err(err) = launch_helper(
        uid,
        &sockets,
        &auth_token,
        &session_id,
        capture_mode,
        &pipeline,
    )
    .await
    {
        warn!(session_id = %session_id, error = %err, "macOS remote desktop helper launch failed");
        cleanup_socket_paths(&sockets);
        remove_failed_pipeline(&session_id, &pipeline, capture_pipelines).await;
        return pipeline;
    }

    tokio::spawn(accept_stream_socket(
        session_id.clone(),
        pipeline.clone(),
        stream_listener,
        auth_token.clone(),
        sockets.stream.clone(),
        capture_mode,
        capture_pipelines.clone(),
        control_pipe_writers.clone(),
    ));
    tokio::spawn(accept_control_socket(
        session_id,
        pipeline.clone(),
        control_pipe_writers,
        control_listener,
        auth_token,
        sockets.control,
        capture_pipelines.clone(),
    ));

    pipeline
}

pub async fn rebuild_capture_pipeline(
    session_id: &str,
    capture_mode: MacosDesktopCaptureMode,
    capture_pipelines: &Arc<RwLock<HashMap<String, Arc<CapturePipeline>>>>,
    control_pipe_writers: &Arc<RwLock<HashMap<String, ControlPipeWriter>>>,
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

    tokio::time::sleep(Duration::from_millis(350)).await;

    for attempt in 1..=8 {
        ensure_capture_pipeline(
            session_id.to_string(),
            capture_mode,
            capture_pipelines,
            control_pipe_writers.clone(),
        )
        .await;

        let has_writer = {
            let guard = control_pipe_writers.read().await;
            guard.contains_key(session_id)
        };
        if has_writer {
            info!(session_id = %session_id, attempt, "macOS pipeline rebuild completed");
            return Ok(());
        }

        warn!(
            session_id = %session_id,
            attempt,
            "macOS pipeline rebuild attempt missing control writer; retrying"
        );
        {
            let mut guard = capture_pipelines.write().await;
            if let Some(existing) = guard.remove(session_id) {
                existing.request_stop();
            }
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }

    Err(anyhow!("macOS pipeline rebuild failed after retries"))
}

pub async fn accept_quic_connections(
    endpoint: Endpoint,
    local_addrs: Vec<LocalAddr>,
    session_id: String,
    punch_sockets: Arc<RwLock<HashMap<String, Arc<UdpSocket>>>>,
    relay_sessions: Arc<RwLock<HashSet<String>>>,
    capture_pipelines: Arc<RwLock<HashMap<String, Arc<CapturePipeline>>>>,
    control_pipe_writers: Arc<RwLock<HashMap<String, ControlPipeWriter>>>,
) -> Result<()> {
    let active_connection: Arc<tokio::sync::Mutex<Option<Connection>>> =
        Arc::new(tokio::sync::Mutex::new(None));

    loop {
        let Some(connecting) = (tokio::select! {
            connecting = endpoint.accept() => connecting,
            _ = sleep(Duration::from_millis(500)) => {
                if !session_transport_is_active(
                    &session_id,
                    &punch_sockets,
                    &relay_sessions,
                    &capture_pipelines,
                ).await {
                    info!(session_id = %session_id, "macOS quic accept loop stopped after session cleanup");
                    break;
                }
                continue;
            }
        }) else {
            break;
        };
        let connection = match connecting.await {
            Ok(conn) => conn,
            Err(err) => {
                warn!(error = %err, "macOS quic connection failed");
                continue;
            }
        };
        {
            let mut guard = active_connection.lock().await;
            if let Some(prev) = guard.take() {
                prev.close(0u32.into(), b"replaced");
            }
            *guard = Some(connection.clone());
        }
        info!(
            session_id = %session_id,
            remote = %connection.remote_address(),
            local_addr_count = local_addrs.len(),
            "macOS quic connection accepted"
        );

        let control_connection = connection.clone();
        let control_writers = control_pipe_writers.clone();
        let control_pipelines = capture_pipelines.clone();
        let control_session_id = session_id.clone();
        tokio::spawn(async move {
            if let Err(err) = read_quic_control_stream(
                control_connection,
                control_session_id,
                control_writers,
                control_pipelines,
            )
            .await
            {
                warn!(error = %err, "macOS quic control stream ended");
            }
        });

        let stream_connection = connection.clone();
        let stream_session_id = session_id.clone();
        let stream_pipelines = capture_pipelines.clone();
        let stream_writers = control_pipe_writers.clone();
        let stream_punch_sockets = punch_sockets.clone();
        let stream_relay_sessions = relay_sessions.clone();
        tokio::spawn(async move {
            let send = match stream_connection.open_uni().await {
                Ok(stream) => stream,
                Err(err) => {
                    warn!(error = %err, "macOS failed to open quic stream");
                    return;
                }
            };
            let pipeline = ensure_capture_pipeline(
                stream_session_id.clone(),
                super::macos_session_capture_mode(&stream_session_id),
                &stream_pipelines,
                stream_writers.clone(),
            )
            .await;
            if let Err(err) = stream_quic_ivf(
                stream_session_id,
                send,
                pipeline,
                stream_punch_sockets,
                stream_relay_sessions,
                stream_pipelines,
                stream_writers,
            )
            .await
            {
                warn!(error = %err, "macOS quic IVF stream failed");
            }
        });
    }
    Ok(())
}

async fn session_transport_is_active(
    session_id: &str,
    punch_sockets: &Arc<RwLock<HashMap<String, Arc<UdpSocket>>>>,
    relay_sessions: &Arc<RwLock<HashSet<String>>>,
    capture_pipelines: &Arc<RwLock<HashMap<String, Arc<CapturePipeline>>>>,
) -> bool {
    if punch_sockets.read().await.contains_key(session_id) {
        return true;
    }
    if relay_sessions.read().await.contains(session_id) {
        return true;
    }
    capture_pipelines.read().await.contains_key(session_id)
}

pub async fn start_relay_client_once(
    session_id: String,
    relay_url: String,
    e2e_key: String,
    relay_sessions: Arc<RwLock<HashSet<String>>>,
    punch_sockets: Arc<RwLock<HashMap<String, Arc<UdpSocket>>>>,
    capture_pipelines: Arc<RwLock<HashMap<String, Arc<CapturePipeline>>>>,
    control_pipe_writers: Arc<RwLock<HashMap<String, ControlPipeWriter>>>,
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
            capture_pipelines,
            control_pipe_writers,
        )
        .await
        {
            warn!(session_id = %session_id, error = %err, "macOS relay client ended unexpectedly");
        }
        relay_sessions.write().await.remove(&session_id);
    });
}

async fn read_authenticated_handshake<R>(stream: &mut R, auth_token: &str) -> Result<()>
where
    R: AsyncRead + Unpin,
{
    read_authenticated_handshake_with_timeout(stream, auth_token, HELPER_HANDSHAKE_TIMEOUT).await
}

async fn read_authenticated_handshake_with_timeout<R>(
    stream: &mut R,
    auth_token: &str,
    read_timeout: Duration,
) -> Result<()>
where
    R: AsyncRead + Unpin,
{
    let mut magic = [0u8; 4];
    read_helper_exact(stream, &mut magic, read_timeout, "handshake magic").await?;
    if magic != HELPER_PIPE_HANDSHAKE_MAGIC {
        return Err(anyhow!("helper handshake magic mismatch"));
    }
    let mut version = [0u8; 2];
    read_helper_exact(stream, &mut version, read_timeout, "handshake version").await?;
    let version = u16::from_be_bytes(version);
    if version != HELPER_PIPE_PROTOCOL_VERSION {
        return Err(anyhow!(
            "helper handshake version mismatch: got {version}, want {HELPER_PIPE_PROTOCOL_VERSION}"
        ));
    }
    let mut len = [0u8; 2];
    read_helper_exact(stream, &mut len, read_timeout, "handshake token length").await?;
    let len = u16::from_be_bytes(len) as usize;
    if len == 0 || len > HELPER_PIPE_MAX_AUTH_TOKEN_LEN {
        return Err(anyhow!("helper handshake token length invalid"));
    }
    let mut token = vec![0u8; len];
    read_helper_exact(stream, &mut token, read_timeout, "handshake token").await?;
    if token != auth_token.as_bytes() {
        return Err(anyhow!("helper handshake token mismatch"));
    }
    Ok(())
}

async fn read_helper_chunk_exact<R>(stream: &mut R, buf: &mut [u8], description: &str) -> Result<()>
where
    R: AsyncRead + Unpin,
{
    read_helper_exact(stream, buf, HELPER_CHUNK_READ_TIMEOUT, description).await
}

async fn read_helper_exact<R>(
    stream: &mut R,
    buf: &mut [u8],
    read_timeout: Duration,
    description: &str,
) -> Result<()>
where
    R: AsyncRead + Unpin,
{
    timeout(read_timeout, stream.read_exact(buf))
        .await
        .map_err(|_| anyhow!("helper {description} read timed out after {read_timeout:?}"))?
        .with_context(|| format!("helper {description} read failed"))?;
    Ok(())
}

async fn accept_stream_socket(
    session_id: String,
    pipeline: Arc<CapturePipeline>,
    listener: UnixListener,
    auth_token: String,
    socket_path: PathBuf,
    capture_mode: MacosDesktopCaptureMode,
    capture_pipelines: Arc<RwLock<HashMap<String, Arc<CapturePipeline>>>>,
    control_pipe_writers: Arc<RwLock<HashMap<String, ControlPipeWriter>>>,
) {
    debug!(
        session_id = %session_id,
        timeout_secs = SOCKET_ACCEPT_TIMEOUT.as_secs(),
        "macOS stream socket waiting for helper"
    );
    let accepted = timeout(SOCKET_ACCEPT_TIMEOUT, listener.accept()).await;
    let (mut stream, _) = match accepted {
        Ok(Ok(value)) => value,
        Ok(Err(err)) => {
            warn!(session_id = %session_id, error = %err, "macOS stream socket accept failed");
            startup_pipeline_failed(
                &session_id,
                &pipeline,
                &capture_pipelines,
                &control_pipe_writers,
                "helper_stream_accept_failed",
                &format!("macOS helper stream socket accept failed: {err}"),
            )
            .await;
            let _ = fs::remove_file(socket_path);
            return;
        }
        Err(_) => {
            warn!(session_id = %session_id, "macOS stream socket accept timed out");
            startup_pipeline_failed(
                &session_id,
                &pipeline,
                &capture_pipelines,
                &control_pipe_writers,
                "helper_stream_accept_timeout",
                "macOS helper stream socket accept timed out",
            )
            .await;
            let _ = fs::remove_file(socket_path);
            return;
        }
    };
    let _ = fs::remove_file(socket_path);
    if let Err(err) = read_authenticated_handshake(&mut stream, &auth_token).await {
        warn!(session_id = %session_id, error = %err, "macOS stream socket auth rejected");
        startup_pipeline_failed(
            &session_id,
            &pipeline,
            &capture_pipelines,
            &control_pipe_writers,
            "helper_stream_auth_rejected",
            &format!("macOS helper stream socket auth rejected: {err}"),
        )
        .await;
        return;
    }

    let mut saw_frame_or_metadata = false;
    let mut saw_complete_display_frame = false;
    loop {
        let mut header = [0u8; 5];
        if let Err(err) = read_helper_chunk_exact(&mut stream, &mut header, "stream header").await {
            if !pipeline.stop_flag().load(Ordering::Relaxed) {
                if capture_mode == MacosDesktopCaptureMode::Screenshot && saw_complete_display_frame
                {
                    info!(
                        session_id = %session_id,
                        error = %err,
                        "macOS screenshot helper stream ended after a complete frame"
                    );
                    break;
                }
                warn!(session_id = %session_id, error = %err, "macOS helper stream ended");
                if !saw_frame_or_metadata {
                    startup_pipeline_failed(
                        &session_id,
                        &pipeline,
                        &capture_pipelines,
                        &control_pipe_writers,
                        "helper_stream_ended_before_frame",
                        &format!("macOS helper stream ended before any frame: {err}"),
                    )
                    .await;
                } else {
                    pipeline_failed(
                        &session_id,
                        &pipeline,
                        &capture_pipelines,
                        &control_pipe_writers,
                        "helper_stream_ended",
                        &format!("macOS helper stream ended unexpectedly: {err}"),
                    )
                    .await;
                }
            }
            break;
        }
        let tag = header[0];
        let len = u32::from_le_bytes([header[1], header[2], header[3], header[4]]) as usize;
        if len > MAX_HELPER_CHUNK_LEN {
            warn!(
                session_id = %session_id,
                tag = tag,
                len = len,
                max_len = MAX_HELPER_CHUNK_LEN,
                "macOS helper stream chunk exceeded safety limit"
            );
            pipeline_failed(
                &session_id,
                &pipeline,
                &capture_pipelines,
                &control_pipe_writers,
                "helper_stream_chunk_too_large",
                "macOS helper stream sent an oversized chunk",
            )
            .await;
            break;
        }
        let mut payload = vec![0u8; len];
        if len > 0 {
            if let Err(err) =
                read_helper_chunk_exact(&mut stream, &mut payload, "stream payload").await
            {
                warn!(
                    session_id = %session_id,
                    tag,
                    len,
                    error = %err,
                    "macOS helper stream payload read failed"
                );
                pipeline_failed(
                    &session_id,
                    &pipeline,
                    &capture_pipelines,
                    &control_pipe_writers,
                    "helper_stream_payload_read_failed",
                    "macOS helper stream payload read failed",
                )
                .await;
                break;
            }
        }
        if should_log_helper_chunk(tag, len) {
            let record_type = (tag == 5).then(|| payload.first().copied()).flatten();
            debug!(
                session_id = %session_id,
                tag,
                len,
                display_record_type = ?record_type,
                display_record = record_type.map(display_record_name).unwrap_or("n/a"),
                "macOS helper stream chunk received"
            );
        }
        match tag {
            0 => {
                saw_frame_or_metadata = true;
                pipeline.push_chunk(talos_worker::encode::IvfChunk::Metadata(payload));
            }
            1 if payload.len() == 32 => {
                let mut header = [0u8; 32];
                header.copy_from_slice(&payload);
                saw_frame_or_metadata = true;
                pipeline.push_chunk(talos_worker::encode::IvfChunk::Header(header));
            }
            2 => {
                saw_frame_or_metadata = true;
                pipeline.push_chunk(talos_worker::encode::IvfChunk::Frame(payload));
            }
            4 => {
                saw_frame_or_metadata = true;
                pipeline.push_chunk(talos_worker::encode::IvfChunk::DisplayKeyframe(payload));
            }
            5 => {
                saw_frame_or_metadata = true;
                if payload.first().copied() == Some(DISPLAY_RECORD_FRAME_END) {
                    saw_complete_display_frame = true;
                }
                pipeline.push_chunk(talos_worker::encode::IvfChunk::DisplayDelta(payload));
            }
            6 => pipeline.touch_helper_alive(),
            7 => {
                let parsed = serde_json::from_slice::<HelperCaptureError>(&payload).ok();
                let reason = parsed
                    .as_ref()
                    .and_then(|value| value.reason.as_deref())
                    .filter(|value| !value.trim().is_empty())
                    .unwrap_or("capture_error");
                let message = parsed
                    .as_ref()
                    .and_then(|value| value.message.as_deref())
                    .filter(|value| !value.trim().is_empty())
                    .unwrap_or("macOS desktop capture failed");
                warn!(session_id = %session_id, reason, message, "macOS helper reported capture failure");
                pipeline.set_failure(reason, message);
                if reason == "screen_recording_denied" {
                    surface_permissions_helper(Some("--remote-desktop-required")).await;
                }
                remove_failed_pipeline_and_control_writer(
                    &session_id,
                    &pipeline,
                    &capture_pipelines,
                    &control_pipe_writers,
                )
                .await;
                break;
            }
            other => {
                warn!(session_id = %session_id, tag = other, len = len, "macOS helper stream sent unknown chunk")
            }
        }
    }
}

pub async fn surface_permissions_helper(reason_arg: Option<&str>) -> bool {
    if !Path::new(PERMISSIONS_HELPER_APP_PATH).exists() {
        return false;
    }
    let uid = match active_console_uid().await {
        Ok(uid) if uid != 0 => uid,
        Ok(_) => return false,
        Err(err) => {
            warn!(error = %err, "macOS permissions helper launch skipped; active console user unavailable");
            return false;
        }
    };
    let mut command = Command::new("/bin/launchctl");
    command
        .arg("asuser")
        .arg(uid.to_string())
        .arg("/usr/bin/open")
        .arg("-na")
        .arg(PERMISSIONS_HELPER_APP_PATH);
    if let Some(arg) = reason_arg {
        command.arg("--args").arg(arg);
    }
    let status = command.status().await;
    match status {
        Ok(status) if status.success() => {
            info!(uid, reason_arg, "macOS permissions helper surfaced");
            true
        }
        Ok(status) => {
            warn!(
                uid,
                status = %status,
                "macOS permissions helper launch failed"
            );
            false
        }
        Err(err) => {
            warn!(
                uid,
                error = %err,
                "macOS permissions helper launch failed"
            );
            false
        }
    }
}

pub async fn wait_for_startup_failure(
    pipeline: &Arc<CapturePipeline>,
) -> Option<super::CaptureFailure> {
    let deadline = Instant::now() + CAPTURE_STARTUP_FAILURE_WAIT;
    loop {
        if let Some(failure) = pipeline.failure() {
            return Some(failure);
        }
        if pipeline.first_frame_at_ms().is_some() || Instant::now() >= deadline {
            return None;
        }
        sleep(Duration::from_millis(50)).await;
    }
}

async fn accept_control_socket(
    session_id: String,
    pipeline: Arc<CapturePipeline>,
    writers: Arc<RwLock<HashMap<String, ControlPipeWriter>>>,
    listener: UnixListener,
    auth_token: String,
    socket_path: PathBuf,
    capture_pipelines: Arc<RwLock<HashMap<String, Arc<CapturePipeline>>>>,
) {
    debug!(
        session_id = %session_id,
        timeout_secs = SOCKET_ACCEPT_TIMEOUT.as_secs(),
        "macOS control socket waiting for helper"
    );
    let accepted = timeout(SOCKET_ACCEPT_TIMEOUT, listener.accept()).await;
    let (mut stream, _) = match accepted {
        Ok(Ok(value)) => value,
        Ok(Err(err)) => {
            warn!(session_id = %session_id, error = %err, "macOS control socket accept failed");
            startup_pipeline_failed(
                &session_id,
                &pipeline,
                &capture_pipelines,
                &writers,
                "helper_control_accept_failed",
                &format!("macOS helper control socket accept failed: {err}"),
            )
            .await;
            let _ = fs::remove_file(socket_path);
            return;
        }
        Err(_) => {
            warn!(session_id = %session_id, "macOS control socket accept timed out");
            startup_pipeline_failed(
                &session_id,
                &pipeline,
                &capture_pipelines,
                &writers,
                "helper_control_accept_timeout",
                "macOS helper control socket accept timed out",
            )
            .await;
            let _ = fs::remove_file(socket_path);
            return;
        }
    };
    let _ = fs::remove_file(socket_path);
    if let Err(err) = read_authenticated_handshake(&mut stream, &auth_token).await {
        warn!(session_id = %session_id, error = %err, "macOS control socket auth rejected");
        startup_pipeline_failed(
            &session_id,
            &pipeline,
            &capture_pipelines,
            &writers,
            "helper_control_auth_rejected",
            &format!("macOS helper control socket auth rejected: {err}"),
        )
        .await;
        return;
    }

    if !pipeline_is_current(&session_id, &pipeline, &capture_pipelines).await {
        info!(
            session_id = %session_id,
            "macOS control socket accepted after pipeline cleanup; dropping stale writer"
        );
        return;
    }

    let (tx, mut rx) = tokio::sync::mpsc::channel::<Vec<u8>>(1024);
    writers
        .write()
        .await
        .insert(session_id.clone(), ControlPipeWriter { tx: tx.clone() });
    while let Some(frame) = rx.recv().await {
        if let Err(err) = stream.write_all(&frame).await {
            if super::macos_session_capture_mode(&session_id) == MacosDesktopCaptureMode::Screenshot
                && pipeline.first_frame_at_ms().is_some()
            {
                info!(
                    session_id = %session_id,
                    error = %err,
                    "macOS screenshot helper control channel closed after frame"
                );
            } else {
                warn!(session_id = %session_id, error = %err, "macOS helper control write failed");
            }
            break;
        }
    }
    remove_control_writer_if_current(&session_id, &tx, &writers).await;
}

async fn startup_pipeline_failed(
    session_id: &str,
    pipeline: &Arc<CapturePipeline>,
    capture_pipelines: &Arc<RwLock<HashMap<String, Arc<CapturePipeline>>>>,
    control_pipe_writers: &Arc<RwLock<HashMap<String, ControlPipeWriter>>>,
    reason: &str,
    message: &str,
) {
    if pipeline.first_frame_at_ms().is_some() {
        pipeline_failed(
            session_id,
            pipeline,
            capture_pipelines,
            control_pipe_writers,
            reason,
            message,
        )
        .await;
        return;
    }
    pipeline.set_failure(reason, message);
    remove_failed_pipeline_and_control_writer(
        session_id,
        pipeline,
        capture_pipelines,
        control_pipe_writers,
    )
    .await;
}

async fn pipeline_failed(
    session_id: &str,
    pipeline: &Arc<CapturePipeline>,
    capture_pipelines: &Arc<RwLock<HashMap<String, Arc<CapturePipeline>>>>,
    control_pipe_writers: &Arc<RwLock<HashMap<String, ControlPipeWriter>>>,
    reason: &str,
    message: &str,
) {
    pipeline.set_failure(reason, message);
    remove_failed_pipeline_and_control_writer(
        session_id,
        pipeline,
        capture_pipelines,
        control_pipe_writers,
    )
    .await;
}

async fn launch_helper(
    uid: u32,
    paths: &SocketPaths,
    auth_token: &str,
    session_id: &str,
    capture_mode: MacosDesktopCaptureMode,
    pipeline: &Arc<CapturePipeline>,
) -> Result<()> {
    let helper = helper_launch_target()?;
    preflight_remote_desktop_permissions(uid, &helper, session_id, pipeline).await?;
    let hide_cursor = super::macos_session_hide_cursor(session_id);
    let mut command = Command::new("/bin/launchctl");
    command.arg("asuser").arg(uid.to_string());
    if let Some(app_path) = helper.app_path.as_ref() {
        command
            .arg("/usr/bin/open")
            .arg("-n")
            .arg("-W")
            .arg(app_path)
            .arg("--args");
    } else {
        command.arg(&helper.executable_path);
    }
    command
        .arg(match capture_mode {
            MacosDesktopCaptureMode::H264 => "capture-macos-h264",
            MacosDesktopCaptureMode::Legacy => "capture-macos-legacy",
            MacosDesktopCaptureMode::Atx2 => "capture-macos-atx2",
            MacosDesktopCaptureMode::Screenshot => "capture-macos-screenshot",
        })
        .arg("--stream-socket")
        .arg(&paths.stream)
        .arg("--control-socket")
        .arg(&paths.control)
        .arg("--auth-token")
        .arg(auth_token)
        .arg("--session-id")
        .arg(session_id)
        .arg("--fps")
        .arg(HELPER_FPS.to_string())
        .env("RMM_HELPER_NO_PAUSE_ON_EXIT", "1");
    if hide_cursor {
        command.arg("--hide-cursor");
    }
    let child = command.spawn().context("spawn launchctl asuser helper")?;
    info!(
        session_id = %session_id,
        uid = uid,
        pid = child.id().unwrap_or_default(),
        capture_mode = ?capture_mode,
        hide_cursor = hide_cursor,
        helper_app = ?helper.app_path,
        helper_executable = %helper.executable_path.display(),
        "macOS remote desktop helper launched"
    );
    Ok(())
}

async fn preflight_remote_desktop_permissions(
    uid: u32,
    helper: &HelperLaunchTarget,
    session_id: &str,
    pipeline: &Arc<CapturePipeline>,
) -> Result<()> {
    match check_helper_permissions(uid, helper).await {
        Ok(snapshot) => {
            if let Some(reason) = remote_desktop_permission_reason(&snapshot) {
                emit_permission_metadata(session_id, reason, &snapshot, pipeline);
                warn!(
                    session_id = %session_id,
                    screen_recording = snapshot.screen_recording,
                    accessibility = snapshot.accessibility,
                    reason,
                    "macOS remote desktop permission preflight reported missing permission; launching capture helper to verify"
                );
                surface_permissions_helper(Some("--remote-desktop-required")).await;
            }
        }
        Err(err) => {
            warn!(
                session_id = %session_id,
                error = %err,
                "macOS remote desktop permission preflight skipped"
            );
        }
    }
    Ok(())
}

fn emit_permission_metadata(
    session_id: &str,
    reason: &str,
    snapshot: &HelperPermissionSnapshot,
    pipeline: &Arc<CapturePipeline>,
) {
    let payload = json!({
        "message_type": "macos_permission_status",
        "reason": reason,
        "screen_recording": snapshot.screen_recording,
        "accessibility": snapshot.accessibility,
        "input_available": snapshot.accessibility,
        "capture_available": snapshot.screen_recording,
        "agent_reported_at_ms": crate::now_unix_ms_u64(),
    });
    let Ok(json_bytes) = serde_json::to_vec(&payload) else {
        warn!(session_id = %session_id, reason, "failed to serialize macOS permission metadata");
        return;
    };
    let mut msg = Vec::with_capacity(8 + json_bytes.len());
    msg.extend_from_slice(b"RMMD");
    msg.extend_from_slice(&(json_bytes.len() as u32).to_le_bytes());
    msg.extend_from_slice(&json_bytes);
    pipeline.push_chunk(talos_worker::encode::IvfChunk::Metadata(msg));
}

async fn check_helper_permissions(
    uid: u32,
    helper: &HelperLaunchTarget,
) -> Result<HelperPermissionSnapshot> {
    if let Some(app_path) = helper.app_path.as_ref() {
        return check_helper_permissions_via_app(uid, app_path).await;
    }
    check_helper_permissions_via_executable(uid, &helper.executable_path).await
}

pub(crate) async fn check_active_console_helper_permissions() -> Result<HelperPermissionSnapshot> {
    let uid = active_console_uid()
        .await
        .and_then(validate_remote_desktop_console_uid)?;
    let helper = helper_launch_target()?;
    check_helper_permissions(uid, &helper).await
}

async fn check_helper_permissions_via_app(
    uid: u32,
    app_path: &Path,
) -> Result<HelperPermissionSnapshot> {
    let output_path = helper_permission_output_path();
    let output = timeout(
        Duration::from_secs(6),
        Command::new("/bin/launchctl")
            .arg("asuser")
            .arg(uid.to_string())
            .arg("/usr/bin/open")
            .arg("-n")
            .arg("-W")
            .arg(app_path)
            .arg("--args")
            .arg("check-macos-permissions")
            .arg("--json")
            .arg("--json-output")
            .arg(&output_path)
            .output(),
    )
    .await
    .context("macOS helper app permission check timed out")?
    .context("run macOS helper app permission check")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let _ = fs::remove_file(&output_path);
        anyhow::bail!(
            "macOS helper app permission check exited with {}: {}",
            output.status,
            stderr.trim()
        );
    }
    let bytes = fs::read(&output_path).with_context(|| {
        format!(
            "read macOS helper app permission output: {}",
            output_path.display()
        )
    })?;
    let _ = fs::remove_file(&output_path);
    serde_json::from_slice::<HelperPermissionSnapshot>(&bytes)
        .context("parse macOS helper app permission check")
}

async fn check_helper_permissions_via_executable(
    uid: u32,
    helper_path: &Path,
) -> Result<HelperPermissionSnapshot> {
    let output = timeout(
        Duration::from_secs(4),
        Command::new("/bin/launchctl")
            .arg("asuser")
            .arg(uid.to_string())
            .arg(helper_path)
            .arg("check-macos-permissions")
            .arg("--json")
            .output(),
    )
    .await
    .context("macOS helper permission check timed out")?
    .context("run macOS helper permission check")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!(
            "macOS helper permission check exited with {}: {}",
            output.status,
            stderr.trim()
        );
    }
    serde_json::from_slice::<HelperPermissionSnapshot>(&output.stdout)
        .context("parse macOS helper permission check")
}

fn helper_permission_output_path() -> PathBuf {
    PathBuf::from("/tmp").join(format!(
        "talos-helper-permissions-{}-{}.json",
        std::process::id(),
        uuid::Uuid::new_v4().simple()
    ))
}

fn remote_desktop_permission_reason(snapshot: &HelperPermissionSnapshot) -> Option<&'static str> {
    if !snapshot.screen_recording {
        Some("screen_recording_denied")
    } else if !snapshot.accessibility {
        Some("accessibility_denied")
    } else {
        None
    }
}

async fn active_console_uid() -> Result<u32> {
    let output = Command::new("/usr/bin/stat")
        .arg("-f")
        .arg("%u")
        .arg("/dev/console")
        .output()
        .await
        .context("query active console uid")?;
    if !output.status.success() {
        return Err(anyhow!("stat /dev/console failed with {}", output.status));
    }
    let raw = String::from_utf8_lossy(&output.stdout);
    raw.trim()
        .parse::<u32>()
        .context("parse active console uid")
}

fn validate_remote_desktop_console_uid(uid: u32) -> Result<u32> {
    if uid == 0 {
        anyhow::bail!("active console user unavailable for macOS remote desktop");
    }
    Ok(uid)
}

fn apply_socket_permissions(paths: &SocketPaths, uid: u32) -> Result<()> {
    apply_socket_path_permissions(&paths.stream, uid)?;
    apply_socket_path_permissions(&paths.control, uid)?;
    Ok(())
}

fn apply_socket_path_permissions(path: &Path, uid: u32) -> Result<()> {
    let c_path = CString::new(path.as_os_str().as_bytes()).with_context(|| {
        format!(
            "prepare macOS remote desktop socket path for chown: {}",
            path.display()
        )
    })?;
    let result = unsafe { libc::chown(c_path.as_ptr(), uid as libc::uid_t, !0 as libc::gid_t) };
    if result != 0 {
        return Err(std::io::Error::last_os_error()).with_context(|| {
            format!(
                "chown macOS remote desktop socket to console uid {uid}: {}",
                path.display()
            )
        });
    }
    fs::set_permissions(path, fs::Permissions::from_mode(socket_permission_mode()))
        .with_context(|| format!("chmod macOS remote desktop socket: {}", path.display()))
}

fn socket_permission_mode() -> u32 {
    0o600
}

#[derive(Clone, Debug)]
struct HelperLaunchTarget {
    app_path: Option<PathBuf>,
    executable_path: PathBuf,
}

fn helper_launch_target() -> Result<HelperLaunchTarget> {
    let app_path = Path::new(WORKER_HELPER_APP_PATH);
    let app_executable = Path::new(WORKER_HELPER_APP_EXECUTABLE);
    if app_path.exists() && app_executable.exists() {
        return Ok(HelperLaunchTarget {
            app_path: Some(app_path.to_path_buf()),
            executable_path: app_executable.to_path_buf(),
        });
    }

    Err(anyhow!(
        "Talos Worker Helper app not found at {}",
        app_path.display()
    ))
}

struct SocketPaths {
    stream: PathBuf,
    control: PathBuf,
}

fn prepare_socket_paths(session_id: &str) -> Result<SocketPaths> {
    let dir = Path::new(MACOS_DESKTOP_SOCKET_DIR);
    fs::create_dir_all(dir).context("create macOS desktop socket dir")?;
    let sanitized: String = session_id
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .take(12)
        .collect();
    let nonce = uuid::Uuid::new_v4().simple().to_string();
    let nonce = &nonce[..12];
    let stream = dir.join(format!("{sanitized}.{nonce}.s"));
    let control = dir.join(format!("{sanitized}.{nonce}.c"));
    ensure_macos_socket_path_fits(&stream)?;
    ensure_macos_socket_path_fits(&control)?;
    let _ = fs::remove_file(&stream);
    let _ = fs::remove_file(&control);
    Ok(SocketPaths { stream, control })
}

fn ensure_macos_socket_path_fits(path: &Path) -> Result<()> {
    use std::os::unix::ffi::OsStrExt;

    let len = path.as_os_str().as_bytes().len();
    if len > MACOS_UNIX_SOCKET_MAX_PATH_BYTES {
        anyhow::bail!(
            "macOS desktop socket path is too long: {} bytes at {}",
            len,
            path.display()
        );
    }
    Ok(())
}

fn cleanup_socket_paths(paths: &SocketPaths) {
    let _ = fs::remove_file(&paths.stream);
    let _ = fs::remove_file(&paths.control);
}

async fn remove_failed_pipeline(
    session_id: &str,
    pipeline: &Arc<CapturePipeline>,
    capture_pipelines: &Arc<RwLock<HashMap<String, Arc<CapturePipeline>>>>,
) -> bool {
    pipeline.request_stop();
    let mut guard = capture_pipelines.write().await;
    if guard
        .get(session_id)
        .is_some_and(|current| Arc::ptr_eq(current, pipeline))
    {
        guard.remove(session_id);
        return true;
    }
    false
}

async fn remove_failed_pipeline_and_control_writer(
    session_id: &str,
    pipeline: &Arc<CapturePipeline>,
    capture_pipelines: &Arc<RwLock<HashMap<String, Arc<CapturePipeline>>>>,
    control_pipe_writers: &Arc<RwLock<HashMap<String, ControlPipeWriter>>>,
) {
    if remove_failed_pipeline(session_id, pipeline, capture_pipelines).await {
        control_pipe_writers.write().await.remove(session_id);
    }
}

async fn remove_control_writer_if_current(
    session_id: &str,
    tx: &tokio::sync::mpsc::Sender<Vec<u8>>,
    control_pipe_writers: &Arc<RwLock<HashMap<String, ControlPipeWriter>>>,
) -> bool {
    let mut guard = control_pipe_writers.write().await;
    if guard
        .get(session_id)
        .is_some_and(|current| current.tx.same_channel(tx))
    {
        guard.remove(session_id);
        return true;
    }
    false
}

async fn pipeline_is_current(
    session_id: &str,
    pipeline: &Arc<CapturePipeline>,
    capture_pipelines: &Arc<RwLock<HashMap<String, Arc<CapturePipeline>>>>,
) -> bool {
    capture_pipelines
        .read()
        .await
        .get(session_id)
        .is_some_and(|current| Arc::ptr_eq(current, pipeline))
}

async fn stream_quic_ivf(
    session_id: String,
    mut send: quinn::SendStream,
    pipeline: Arc<CapturePipeline>,
    punch_sockets: Arc<RwLock<HashMap<String, Arc<UdpSocket>>>>,
    relay_sessions: Arc<RwLock<HashSet<String>>>,
    capture_pipelines: Arc<RwLock<HashMap<String, Arc<CapturePipeline>>>>,
    control_pipe_writers: Arc<RwLock<HashMap<String, ControlPipeWriter>>>,
) -> Result<()> {
    pipeline.start_stream();
    let mut rx = pipeline.subscribe();
    let mut last_seq = None;
    for item in frame_safe_snapshot(pipeline.snapshot()) {
        send_quic_chunk(&mut send, &item.chunk).await?;
        last_seq = Some(item.seq);
    }
    let stop_flag = pipeline.stop_flag();
    loop {
        tokio::select! {
            result = rx.recv() => {
                let seq = match result {
                    Ok(seq) => seq,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                };
                if last_seq.is_some_and(|last| seq <= last) {
                    continue;
                }
                let Some(chunk) = find_buffered_chunk(&pipeline, seq) else { continue; };
                if display_delta_record_type(&chunk).is_some() {
                    let min_seq = last_seq.map_or(0, |last| last.saturating_add(1));
                    let Some(frame) = buffered_display_frame_from(&pipeline, min_seq) else {
                        continue;
                    };
                    last_seq = send_quic_buffered_chunks(&mut send, &frame).await?;
                    relay_sessions.write().await.remove(&session_id);
                    continue;
                }
                send_quic_chunk(&mut send, &chunk).await?;
                if is_live_frame_chunk(&chunk) {
                    relay_sessions.write().await.remove(&session_id);
                }
                last_seq = Some(seq);
            }
            _ = sleep(Duration::from_millis(500)) => {
                if stop_flag.load(Ordering::SeqCst) {
                    break;
                }
            }
        }
    }
    let _ = send.finish();
    stop_pipeline_if_idle(
        &session_id,
        &pipeline,
        &capture_pipelines,
        &control_pipe_writers,
        &punch_sockets,
        &relay_sessions,
    )
    .await;
    Ok(())
}

fn is_live_frame_chunk(chunk: &talos_worker::encode::IvfChunk) -> bool {
    matches!(
        chunk,
        talos_worker::encode::IvfChunk::Frame(_)
            | talos_worker::encode::IvfChunk::DisplayKeyframe(_)
            | talos_worker::encode::IvfChunk::DisplayDelta(_)
    )
}

fn display_delta_record_type(chunk: &talos_worker::encode::IvfChunk) -> Option<u8> {
    match chunk {
        talos_worker::encode::IvfChunk::DisplayDelta(bytes) => bytes.first().copied(),
        _ => None,
    }
}

fn display_record_name(record_type: u8) -> &'static str {
    match record_type {
        DISPLAY_RECORD_FRAME_BEGIN => "frame_begin",
        DISPLAY_RECORD_KEYFRAME => "keyframe",
        DISPLAY_RECORD_FRAME_END => "frame_end",
        _ => "other",
    }
}

fn should_log_helper_chunk(tag: u8, len: usize) -> bool {
    tag == 0 || tag == 5 || tag == 4 || len >= 1024 * 1024
}

fn ivf_chunk_bytes(chunk: &talos_worker::encode::IvfChunk) -> usize {
    match chunk {
        talos_worker::encode::IvfChunk::Metadata(bytes) => bytes.len(),
        talos_worker::encode::IvfChunk::Header(bytes) => bytes.len(),
        talos_worker::encode::IvfChunk::Frame(bytes)
        | talos_worker::encode::IvfChunk::DisplayKeyframe(bytes)
        | talos_worker::encode::IvfChunk::DisplayDelta(bytes) => bytes.len(),
    }
}

fn ivf_chunk_kind(chunk: &talos_worker::encode::IvfChunk) -> &'static str {
    match chunk {
        talos_worker::encode::IvfChunk::Metadata(_) => "metadata",
        talos_worker::encode::IvfChunk::Header(_) => "legacy_header",
        talos_worker::encode::IvfChunk::Frame(_) => "legacy_frame",
        talos_worker::encode::IvfChunk::DisplayKeyframe(_) => "display_keyframe",
        talos_worker::encode::IvfChunk::DisplayDelta(_) => "display_delta",
    }
}

fn is_display_frame_begin(chunk: &talos_worker::encode::IvfChunk) -> bool {
    display_delta_record_type(chunk) == Some(DISPLAY_RECORD_FRAME_BEGIN)
}

fn is_display_frame_end(chunk: &talos_worker::encode::IvfChunk) -> bool {
    display_delta_record_type(chunk) == Some(DISPLAY_RECORD_FRAME_END)
}

fn buffered_display_frame_from(
    pipeline: &CapturePipeline,
    min_seq: u64,
) -> Option<Vec<crate::BufferedChunk>> {
    let guard = pipeline.buffer.lock().ok()?;
    let mut frame = Vec::new();
    let mut collecting = false;
    for item in guard.iter().filter(|item| item.seq >= min_seq) {
        if is_display_frame_begin(&item.chunk) {
            frame.clear();
            frame.push(item.clone());
            collecting = true;
            continue;
        }
        if !collecting {
            continue;
        }
        frame.push(item.clone());
        if is_display_frame_end(&item.chunk) {
            return Some(frame);
        }
    }
    None
}

async fn send_quic_buffered_chunks(
    send: &mut quinn::SendStream,
    chunks: &[crate::BufferedChunk],
) -> Result<Option<u64>> {
    let mut last_seq = None;
    for item in chunks {
        send_quic_chunk(send, &item.chunk).await?;
        last_seq = Some(item.seq);
    }
    Ok(last_seq)
}

async fn send_relay_buffered_chunks<W>(
    session_id: &str,
    stream: &mut W,
    cipher: &ChaCha20Poly1305,
    send_counter: &mut u64,
    chunks: &[crate::BufferedChunk],
) -> Result<Option<u64>>
where
    W: AsyncWriteExt + Unpin,
{
    let mut last_seq = None;
    for item in chunks {
        let record_type = display_delta_record_type(&item.chunk);
        debug!(
            session_id = %session_id,
            seq = item.seq,
            chunk_kind = ivf_chunk_kind(&item.chunk),
            bytes = ivf_chunk_bytes(&item.chunk),
            display_record_type = ?record_type,
            display_record = record_type.map(display_record_name).unwrap_or("n/a"),
            "macOS relay sending buffered chunk"
        );
        send_relay_chunk(stream, cipher, send_counter, &item.chunk).await?;
        debug!(
            session_id = %session_id,
            seq = item.seq,
            chunk_kind = ivf_chunk_kind(&item.chunk),
            bytes = ivf_chunk_bytes(&item.chunk),
            display_record_type = ?record_type,
            display_record = record_type.map(display_record_name).unwrap_or("n/a"),
            "macOS relay sent buffered chunk"
        );
        last_seq = Some(item.seq);
    }
    Ok(last_seq)
}

fn frame_safe_snapshot(snapshot: Vec<crate::BufferedChunk>) -> Vec<crate::BufferedChunk> {
    let mut safe = Vec::with_capacity(snapshot.len());
    let mut pending_display_frame = Vec::new();
    let mut collecting_display_frame = false;

    for item in snapshot {
        if display_delta_record_type(&item.chunk).is_some() {
            if is_display_frame_begin(&item.chunk) {
                pending_display_frame.clear();
                pending_display_frame.push(item);
                collecting_display_frame = true;
                continue;
            }
            if collecting_display_frame {
                let frame_done = is_display_frame_end(&item.chunk);
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

async fn stream_relay_ivf<W>(
    session_id: String,
    stream: &mut W,
    cipher: &ChaCha20Poly1305,
    send_counter: &mut u64,
    pipeline: Arc<CapturePipeline>,
    punch_sockets: Arc<RwLock<HashMap<String, Arc<UdpSocket>>>>,
    relay_sessions: Arc<RwLock<HashSet<String>>>,
    capture_pipelines: Arc<RwLock<HashMap<String, Arc<CapturePipeline>>>>,
    control_pipe_writers: Arc<RwLock<HashMap<String, ControlPipeWriter>>>,
) -> Result<()>
where
    W: AsyncWriteExt + Unpin,
{
    pipeline.start_stream();
    let mut rx = pipeline.subscribe();
    let mut last_seq = None;
    let mut last_send_at = Instant::now();
    let snapshot = frame_safe_snapshot(pipeline.snapshot());
    debug!(
        session_id = %session_id,
        chunks = snapshot.len(),
        bytes = snapshot
            .iter()
            .map(|item| ivf_chunk_bytes(&item.chunk))
            .sum::<usize>(),
        "macOS relay sending initial capture snapshot"
    );
    for item in snapshot {
        let record_type = display_delta_record_type(&item.chunk);
        if record_type.is_some()
            || matches!(item.chunk, talos_worker::encode::IvfChunk::Metadata(_))
            || ivf_chunk_bytes(&item.chunk) >= 1024 * 1024
        {
            debug!(
                session_id = %session_id,
                seq = item.seq,
                chunk_kind = ivf_chunk_kind(&item.chunk),
                bytes = ivf_chunk_bytes(&item.chunk),
                display_record_type = ?record_type,
                display_record = record_type.map(display_record_name).unwrap_or("n/a"),
                "macOS relay sending snapshot chunk"
            );
        }
        send_relay_chunk(stream, cipher, send_counter, &item.chunk).await?;
        last_seq = Some(item.seq);
    }
    let stop_flag = pipeline.stop_flag();
    loop {
        tokio::select! {
            result = rx.recv() => {
                let seq = match result {
                    Ok(seq) => seq,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                };
                if last_seq.is_some_and(|last| seq <= last) {
                    continue;
                }
                let Some(chunk) = find_buffered_chunk(&pipeline, seq) else { continue; };
                if display_delta_record_type(&chunk).is_some() {
                    let min_seq = last_seq.map_or(0, |last| last.saturating_add(1));
                    let Some(frame) = buffered_display_frame_from(&pipeline, min_seq) else {
                        continue;
                    };
                    debug!(
                        session_id = %session_id,
                        min_seq,
                        chunks = frame.len(),
                        bytes = frame
                            .iter()
                            .map(|item| ivf_chunk_bytes(&item.chunk))
                            .sum::<usize>(),
                        first_seq = frame.first().map(|item| item.seq),
                        last_seq = frame.last().map(|item| item.seq),
                        "macOS relay sending complete display frame"
                    );
                    last_seq =
                        send_relay_buffered_chunks(&session_id, stream, cipher, send_counter, &frame)
                            .await?;
                    last_send_at = Instant::now();
                    continue;
                }
                send_relay_chunk(stream, cipher, send_counter, &chunk).await?;
                last_seq = Some(seq);
                last_send_at = Instant::now();
            }
            _ = sleep(Duration::from_millis(500)) => {
                if !relay_sessions.read().await.contains(&session_id) || stop_flag.load(Ordering::SeqCst) {
                    break;
                }
                if last_send_at.elapsed() >= Duration::from_secs(10) {
                    debug!(session_id = %session_id, "macOS relay IVF stream idle");
                    last_send_at = Instant::now();
                }
            }
        }
    }
    stop_pipeline_if_idle(
        &session_id,
        &pipeline,
        &capture_pipelines,
        &control_pipe_writers,
        &punch_sockets,
        &relay_sessions,
    )
    .await;
    Ok(())
}

async fn run_relay_client(
    session_id: String,
    relay_url: String,
    e2e_key_b64: String,
    punch_sockets: Arc<RwLock<HashMap<String, Arc<UdpSocket>>>>,
    relay_sessions: Arc<RwLock<HashSet<String>>>,
    capture_pipelines: Arc<RwLock<HashMap<String, Arc<CapturePipeline>>>>,
    control_pipe_writers: Arc<RwLock<HashMap<String, ControlPipeWriter>>>,
) -> Result<()> {
    let relay_target = parse_relay_target(&relay_url)?;
    debug!(
        session_id = %session_id,
        relay_host = %relay_target.host,
        relay_port = relay_target.port,
        "macOS relay client connecting"
    );
    let tcp_stream = timeout(
        Duration::from_secs(10),
        TcpStream::connect(format!("{}:{}", relay_target.host, relay_target.port)),
    )
    .await
    .map_err(|_| anyhow!("connect relay tcp timed out"))?
    .context("connect relay tcp")?;
    tcp_stream.set_nodelay(true)?;
    debug!(
        session_id = %session_id,
        relay_host = %relay_target.host,
        relay_port = relay_target.port,
        "macOS relay TCP connected"
    );

    let connector = TlsConnector::from(Arc::new(build_relay_client_tls_config(None, None)?));
    let server_name = ServerName::try_from(relay_target.host.clone())?;
    let mut stream = timeout(
        Duration::from_secs(10),
        connector.connect(server_name, tcp_stream),
    )
    .await
    .map_err(|_| anyhow!("relay tls connect timed out"))??;
    debug!(
        session_id = %session_id,
        relay_host = %relay_target.host,
        "macOS relay TLS connected"
    );
    let request = format!(
        "GET /relay/{session_id} HTTP/1.1\r\nHost: {host}\r\n\r\n",
        host = relay_target.host
    );
    stream.write_all(request.as_bytes()).await?;
    timeout(Duration::from_secs(10), read_http_response(&mut stream))
        .await
        .map_err(|_| anyhow!("read relay response timed out"))??;
    debug!(session_id = %session_id, "macOS relay HTTP response read");

    let key_bytes = base64::engine::general_purpose::STANDARD
        .decode(e2e_key_b64.trim())
        .context("decode relay e2e key")?;
    let cipher = build_e2e_cipher(&key_bytes)?;
    let mut send_counter = 0u64;
    write_e2e_frame(&mut stream, &cipher, &mut send_counter, b"hello-world").await?;
    debug!(session_id = %session_id, "macOS relay hello sent");

    let capture_mode = super::macos_session_capture_mode(&session_id);
    debug!(
        session_id = %session_id,
        capture_mode = ?capture_mode,
        "macOS relay ensuring capture pipeline"
    );
    let pipeline = ensure_capture_pipeline(
        session_id.clone(),
        capture_mode,
        &capture_pipelines,
        control_pipe_writers.clone(),
    )
    .await;
    let (reader, mut writer) = tokio::io::split(stream);
    let cipher_read = build_e2e_cipher(&key_bytes)?;
    tokio::spawn(run_heartbeat_read_loop(
        session_id.clone(),
        reader,
        cipher_read,
        pipeline.clone(),
        control_pipe_writers.clone(),
        capture_pipelines.clone(),
    ));
    stream_relay_ivf(
        session_id,
        &mut writer,
        &cipher,
        &mut send_counter,
        pipeline,
        punch_sockets,
        relay_sessions,
        capture_pipelines,
        control_pipe_writers,
    )
    .await
}

async fn read_quic_control_stream(
    connection: Connection,
    session_id: String,
    control_pipe_writers: Arc<RwLock<HashMap<String, ControlPipeWriter>>>,
    capture_pipelines: Arc<RwLock<HashMap<String, Arc<CapturePipeline>>>>,
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
        )
        .await;
    }
    Ok(())
}

async fn run_heartbeat_read_loop<R>(
    session_id: String,
    mut reader: R,
    cipher: ChaCha20Poly1305,
    pipeline: Arc<CapturePipeline>,
    control_pipe_writers: Arc<RwLock<HashMap<String, ControlPipeWriter>>>,
    capture_pipelines: Arc<RwLock<HashMap<String, Arc<CapturePipeline>>>>,
) where
    R: AsyncReadExt + Unpin + Send,
{
    let heartbeat_timeout = Duration::from_secs(HEARTBEAT_INTERVAL_SECS + 2);
    let mut missed = 0u32;
    while missed < HEARTBEAT_MISSED_THRESHOLD {
        match timeout(heartbeat_timeout, read_e2e_frame_from(&mut reader, &cipher)).await {
            Ok(Ok(payload)) if payload == HEARTBEAT_PAYLOAD || payload == b"hello-world" => {
                missed = 0;
            }
            Ok(Ok(payload)) => match parse_control_frame(&payload) {
                Ok(frame) => {
                    dispatch_control_message(
                        &session_id,
                        frame.message_type,
                        frame.payload,
                        &control_pipe_writers,
                        &capture_pipelines,
                    )
                    .await;
                }
                Err(err) => {
                    warn!(session_id = %session_id, error = %err, "macOS invalid relay control frame")
                }
            },
            Ok(Err(err)) => {
                if is_relay_connection_closed(&err) {
                    break;
                }
                missed += 1;
            }
            Err(_) => missed += 1,
        }
    }
    pipeline.request_stop();
}

async fn dispatch_control_message(
    session_id: &str,
    message_type: u8,
    payload: &[u8],
    control_pipe_writers: &Arc<RwLock<HashMap<String, ControlPipeWriter>>>,
    capture_pipelines: &Arc<RwLock<HashMap<String, Arc<CapturePipeline>>>>,
) {
    if message_type == CONTROL_TYPE_CONNECTION_PING {
        if payload.len() == CONTROL_PAYLOAD_TIMESTAMP_LEN {
            let echoed_at_ms = u64::from_be_bytes(payload.try_into().unwrap());
            emit_connection_pong_metadata(session_id, echoed_at_ms, capture_pipelines).await;
        } else {
            warn!(
                session_id = %session_id,
                payload_len = payload.len(),
                "invalid macOS connection ping payload"
            );
        }
        return;
    }
    if matches!(
        message_type,
        CONTROL_TYPE_REGISTRY_REQUEST
            | CONTROL_TYPE_SESSION_SWITCH
            | CONTROL_TYPE_SESSION_LOGOFF
            | CONTROL_TYPE_SECURE_ATTENTION
    ) {
        info!(session_id = %session_id, message_type = message_type, "macOS ignored unsupported Windows-only control message");
        return;
    }
    if let Some(writer) = control_pipe_writers.read().await.get(session_id).cloned() {
        match build_control_frame(message_type, payload) {
            Ok(frame) => {
                super::enqueue_helper_control_frame(session_id, message_type, frame, &writer).await;
            }
            Err(err) => {
                warn!(session_id = %session_id, error = %err, "macOS failed to encode helper control frame")
            }
        }
    }
}

async fn emit_connection_pong_metadata(
    session_id: &str,
    echoed_at_ms: u64,
    capture_pipelines: &Arc<RwLock<HashMap<String, Arc<CapturePipeline>>>>,
) {
    let payload = json!({
        "message_type": "connection_pong",
        "echoed_at_ms": echoed_at_ms,
        "agent_received_at_ms": super::now_unix_ms().min(u128::from(u64::MAX)) as u64,
    });
    let Ok(json_bytes) = serde_json::to_vec(&payload) else {
        return;
    };
    let mut msg = Vec::with_capacity(8 + json_bytes.len());
    msg.extend_from_slice(b"RMMD");
    msg.extend_from_slice(&(json_bytes.len() as u32).to_le_bytes());
    msg.extend_from_slice(&json_bytes);
    if let Some(pipeline) = capture_pipelines.read().await.get(session_id).cloned() {
        pipeline.push_chunk(talos_worker::encode::IvfChunk::Metadata(msg));
    }
}

async fn stop_pipeline_if_idle(
    session_id: &str,
    pipeline: &Arc<CapturePipeline>,
    capture_pipelines: &Arc<RwLock<HashMap<String, Arc<CapturePipeline>>>>,
    control_pipe_writers: &Arc<RwLock<HashMap<String, ControlPipeWriter>>>,
    punch_sockets: &Arc<RwLock<HashMap<String, Arc<UdpSocket>>>>,
    relay_sessions: &Arc<RwLock<HashSet<String>>>,
) {
    let should_stop = pipeline.finish_stream();
    if !should_stop {
        return;
    }
    if capture_pipelines
        .read()
        .await
        .get(session_id)
        .is_some_and(|current| !Arc::ptr_eq(current, pipeline))
    {
        pipeline.request_stop();
        return;
    }
    if let Some(writer) = control_pipe_writers.read().await.get(session_id).cloned() {
        let _ = writer.tx.send(vec![0, 0, CONTROL_TYPE_STOP_CAPTURE]).await;
        sleep(Duration::from_millis(200)).await;
    }
    pipeline.request_stop();
    capture_pipelines.write().await.remove(session_id);
    control_pipe_writers.write().await.remove(session_id);
    punch_sockets.write().await.remove(session_id);
    relay_sessions.write().await.remove(session_id);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::ffi::OsStrExt;

    #[test]
    fn prepares_short_macos_socket_paths() {
        let paths =
            prepare_socket_paths("cf432ef5-b906-4506-b969-7c682dc94814").expect("socket paths");

        let stream_len = paths.stream.as_os_str().as_bytes().len();
        let control_len = paths.control.as_os_str().as_bytes().len();

        assert!(
            stream_len <= MACOS_UNIX_SOCKET_MAX_PATH_BYTES,
            "stream socket path too long: {}",
            paths.stream.display()
        );
        assert!(
            control_len <= MACOS_UNIX_SOCKET_MAX_PATH_BYTES,
            "control socket path too long: {}",
            paths.control.display()
        );
        assert_eq!(
            paths.stream.parent(),
            Some(Path::new(MACOS_DESKTOP_SOCKET_DIR))
        );
        assert_eq!(
            paths.control.parent(),
            Some(Path::new(MACOS_DESKTOP_SOCKET_DIR))
        );

        cleanup_socket_paths(&paths);
    }

    #[test]
    fn remote_desktop_permission_reason_prioritizes_screen_recording() {
        assert_eq!(
            remote_desktop_permission_reason(&HelperPermissionSnapshot {
                accessibility: false,
                screen_recording: false,
            }),
            Some("screen_recording_denied")
        );
        assert_eq!(
            remote_desktop_permission_reason(&HelperPermissionSnapshot {
                accessibility: false,
                screen_recording: true,
            }),
            Some("accessibility_denied")
        );
        assert_eq!(
            remote_desktop_permission_reason(&HelperPermissionSnapshot {
                accessibility: true,
                screen_recording: true,
            }),
            None
        );
    }

    #[test]
    fn remote_desktop_rejects_root_console_uid() {
        assert_eq!(validate_remote_desktop_console_uid(501).unwrap(), 501);

        let err = validate_remote_desktop_console_uid(0)
            .expect_err("root console uid should not launch capture helper");
        assert!(err.to_string().contains("active console user unavailable"));
    }

    #[test]
    fn remote_desktop_helper_sockets_are_console_user_private() {
        assert_eq!(socket_permission_mode(), 0o600);
    }

    #[test]
    fn permission_metadata_reports_input_and_capture_availability() {
        let pipeline = Arc::new(CapturePipeline::new());
        let snapshot = HelperPermissionSnapshot {
            accessibility: false,
            screen_recording: true,
        };

        emit_permission_metadata("session-1", "accessibility_denied", &snapshot, &pipeline);

        let chunks = pipeline.snapshot();
        assert_eq!(chunks.len(), 1);
        let talos_worker::encode::IvfChunk::Metadata(bytes) = &chunks[0].chunk else {
            panic!("expected metadata chunk");
        };
        assert!(bytes.starts_with(b"RMMD"));
        let len = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]) as usize;
        let payload: serde_json::Value =
            serde_json::from_slice(&bytes[8..8 + len]).expect("metadata json");

        assert_eq!(payload["message_type"], "macos_permission_status");
        assert_eq!(payload["reason"], "accessibility_denied");
        assert_eq!(payload["screen_recording"], true);
        assert_eq!(payload["accessibility"], false);
        assert_eq!(payload["capture_available"], true);
        assert_eq!(payload["input_available"], false);
    }

    #[tokio::test]
    async fn dispatch_connection_ping_emits_pong_metadata() {
        let session_id = "session-ping";
        let pipeline = Arc::new(CapturePipeline::new());
        let capture_pipelines = Arc::new(RwLock::new(HashMap::new()));
        capture_pipelines
            .write()
            .await
            .insert(session_id.to_string(), pipeline.clone());
        let control_pipe_writers = Arc::new(RwLock::new(HashMap::new()));
        let echoed_at_ms = 123_456_789u64;

        dispatch_control_message(
            session_id,
            CONTROL_TYPE_CONNECTION_PING,
            &echoed_at_ms.to_be_bytes(),
            &control_pipe_writers,
            &capture_pipelines,
        )
        .await;

        let chunks = pipeline.snapshot();
        assert_eq!(chunks.len(), 1);
        let talos_worker::encode::IvfChunk::Metadata(bytes) = &chunks[0].chunk else {
            panic!("expected metadata chunk");
        };
        assert!(bytes.starts_with(b"RMMD"));
        let len = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]) as usize;
        let payload: serde_json::Value =
            serde_json::from_slice(&bytes[8..8 + len]).expect("metadata json");

        assert_eq!(payload["message_type"], "connection_pong");
        assert_eq!(payload["echoed_at_ms"], echoed_at_ms);
        assert!(payload["agent_received_at_ms"].as_u64().is_some());
    }

    #[tokio::test]
    async fn dispatch_connection_ping_rejects_bad_payload_length() {
        let session_id = "session-ping-bad";
        let pipeline = Arc::new(CapturePipeline::new());
        let capture_pipelines = Arc::new(RwLock::new(HashMap::new()));
        capture_pipelines
            .write()
            .await
            .insert(session_id.to_string(), pipeline.clone());
        let control_pipe_writers = Arc::new(RwLock::new(HashMap::new()));

        dispatch_control_message(
            session_id,
            CONTROL_TYPE_CONNECTION_PING,
            &[1, 2, 3],
            &control_pipe_writers,
            &capture_pipelines,
        )
        .await;

        assert!(pipeline.snapshot().is_empty());
    }

    #[tokio::test]
    async fn failed_pipeline_cleanup_removes_control_writer() {
        let session_id = "session-cleanup";
        let pipeline = Arc::new(CapturePipeline::new());
        let capture_pipelines = Arc::new(RwLock::new(HashMap::new()));
        capture_pipelines
            .write()
            .await
            .insert(session_id.to_string(), pipeline.clone());
        let control_pipe_writers = Arc::new(RwLock::new(HashMap::new()));
        let (tx, _rx) = tokio::sync::mpsc::channel(1);
        control_pipe_writers
            .write()
            .await
            .insert(session_id.to_string(), ControlPipeWriter { tx });

        remove_failed_pipeline_and_control_writer(
            session_id,
            &pipeline,
            &capture_pipelines,
            &control_pipe_writers,
        )
        .await;

        assert!(!capture_pipelines.read().await.contains_key(session_id));
        assert!(!control_pipe_writers.read().await.contains_key(session_id));
    }

    #[tokio::test]
    async fn stale_pipeline_cleanup_preserves_replacement_control_writer() {
        let session_id = "session-stale-cleanup";
        let stale_pipeline = Arc::new(CapturePipeline::new());
        let replacement_pipeline = Arc::new(CapturePipeline::new());
        let capture_pipelines = Arc::new(RwLock::new(HashMap::new()));
        capture_pipelines
            .write()
            .await
            .insert(session_id.to_string(), replacement_pipeline);
        let control_pipe_writers = Arc::new(RwLock::new(HashMap::new()));
        let (replacement_tx, _replacement_rx) = tokio::sync::mpsc::channel(1);
        let replacement_tx_for_assert = replacement_tx.clone();
        control_pipe_writers.write().await.insert(
            session_id.to_string(),
            ControlPipeWriter { tx: replacement_tx },
        );

        remove_failed_pipeline_and_control_writer(
            session_id,
            &stale_pipeline,
            &capture_pipelines,
            &control_pipe_writers,
        )
        .await;

        let guard = control_pipe_writers.read().await;
        let writer = guard.get(session_id).expect("replacement writer remains");
        assert!(writer.tx.same_channel(&replacement_tx_for_assert));
    }

    #[tokio::test]
    async fn stale_control_writer_cleanup_preserves_replacement_sender() {
        let session_id = "session-writer-cleanup";
        let control_pipe_writers = Arc::new(RwLock::new(HashMap::new()));
        let (stale_tx, _stale_rx) = tokio::sync::mpsc::channel(1);
        let (replacement_tx, _replacement_rx) = tokio::sync::mpsc::channel(1);
        let replacement_tx_for_assert = replacement_tx.clone();
        control_pipe_writers.write().await.insert(
            session_id.to_string(),
            ControlPipeWriter { tx: replacement_tx },
        );

        assert!(
            !remove_control_writer_if_current(session_id, &stale_tx, &control_pipe_writers).await
        );
        let guard = control_pipe_writers.read().await;
        let writer = guard.get(session_id).expect("replacement writer remains");
        assert!(writer.tx.same_channel(&replacement_tx_for_assert));
    }

    #[tokio::test]
    async fn pipeline_current_check_rejects_stale_pipeline() {
        let session_id = "session-current";
        let pipeline = Arc::new(CapturePipeline::new());
        let replacement = Arc::new(CapturePipeline::new());
        let capture_pipelines = Arc::new(RwLock::new(HashMap::new()));
        capture_pipelines
            .write()
            .await
            .insert(session_id.to_string(), replacement);

        assert!(!pipeline_is_current(session_id, &pipeline, &capture_pipelines).await);
    }

    #[tokio::test]
    async fn session_transport_activity_tracks_cleanup_maps() {
        let session_id = "session-1";
        let punch_sockets = Arc::new(RwLock::new(HashMap::new()));
        let relay_sessions = Arc::new(RwLock::new(HashSet::new()));
        let capture_pipelines = Arc::new(RwLock::new(HashMap::new()));

        assert!(
            !session_transport_is_active(
                session_id,
                &punch_sockets,
                &relay_sessions,
                &capture_pipelines,
            )
            .await
        );

        capture_pipelines
            .write()
            .await
            .insert(session_id.to_string(), Arc::new(CapturePipeline::new()));
        assert!(
            session_transport_is_active(
                session_id,
                &punch_sockets,
                &relay_sessions,
                &capture_pipelines,
            )
            .await
        );

        capture_pipelines.write().await.remove(session_id);
        relay_sessions.write().await.insert(session_id.to_string());
        assert!(
            session_transport_is_active(
                session_id,
                &punch_sockets,
                &relay_sessions,
                &capture_pipelines,
            )
            .await
        );

        relay_sessions.write().await.remove(session_id);
        let socket = UdpSocket::bind("127.0.0.1:0").expect("bind test UDP socket");
        punch_sockets
            .write()
            .await
            .insert(session_id.to_string(), Arc::new(socket));
        assert!(
            session_transport_is_active(
                session_id,
                &punch_sockets,
                &relay_sessions,
                &capture_pipelines,
            )
            .await
        );

        punch_sockets.write().await.remove(session_id);
        assert!(
            !session_transport_is_active(
                session_id,
                &punch_sockets,
                &relay_sessions,
                &capture_pipelines,
            )
            .await
        );
    }

    #[tokio::test]
    async fn helper_handshake_accepts_authenticated_peer() {
        let token = "token-123";
        let (mut client, mut server) = tokio::io::duplex(128);
        let mut handshake = Vec::new();
        handshake.extend_from_slice(&HELPER_PIPE_HANDSHAKE_MAGIC);
        handshake.extend_from_slice(&HELPER_PIPE_PROTOCOL_VERSION.to_be_bytes());
        handshake.extend_from_slice(&(token.len() as u16).to_be_bytes());
        handshake.extend_from_slice(token.as_bytes());

        let writer = tokio::spawn(async move {
            client.write_all(&handshake).await.expect("write handshake");
        });

        read_authenticated_handshake_with_timeout(&mut server, token, Duration::from_millis(100))
            .await
            .expect("authenticated handshake");
        writer.await.expect("writer task");
    }

    #[tokio::test]
    async fn helper_handshake_times_out_for_stalled_peer() {
        let (_client, mut server) = tokio::io::duplex(128);

        let err = read_authenticated_handshake_with_timeout(
            &mut server,
            "token-123",
            Duration::from_millis(10),
        )
        .await
        .expect_err("stalled helper should time out");

        assert!(err.to_string().contains("timed out"));
    }

    #[test]
    fn live_frame_chunk_recognizes_legacy_and_atx2_frames() {
        assert!(is_live_frame_chunk(&talos_worker::encode::IvfChunk::Frame(
            vec![1, 2, 3]
        )));
        assert!(is_live_frame_chunk(
            &talos_worker::encode::IvfChunk::DisplayKeyframe(vec![4, 5, 6])
        ));
        assert!(is_live_frame_chunk(
            &talos_worker::encode::IvfChunk::DisplayDelta(vec![7, 8, 9])
        ));
    }

    #[test]
    fn live_frame_chunk_ignores_metadata_and_headers() {
        assert!(!is_live_frame_chunk(
            &talos_worker::encode::IvfChunk::Metadata(vec![1, 2, 3])
        ));
        assert!(!is_live_frame_chunk(
            &talos_worker::encode::IvfChunk::Header([0; 32])
        ));
    }

    #[test]
    fn frame_safe_snapshot_keeps_complete_atx2_frames() {
        let snapshot = vec![
            crate::BufferedChunk {
                seq: 1,
                chunk: talos_worker::encode::IvfChunk::Metadata(vec![0]),
            },
            crate::BufferedChunk {
                seq: 2,
                chunk: talos_worker::encode::IvfChunk::DisplayDelta(vec![
                    DISPLAY_RECORD_FRAME_BEGIN,
                ]),
            },
            crate::BufferedChunk {
                seq: 3,
                chunk: talos_worker::encode::IvfChunk::DisplayDelta(vec![0x02]),
            },
            crate::BufferedChunk {
                seq: 4,
                chunk: talos_worker::encode::IvfChunk::DisplayDelta(vec![DISPLAY_RECORD_FRAME_END]),
            },
        ];

        let safe = frame_safe_snapshot(snapshot);

        assert_eq!(safe.len(), 4);
        assert_eq!(safe.last().map(|item| item.seq), Some(4));
    }

    #[test]
    fn frame_safe_snapshot_drops_partial_atx2_frames() {
        let snapshot = vec![
            crate::BufferedChunk {
                seq: 1,
                chunk: talos_worker::encode::IvfChunk::Metadata(vec![0]),
            },
            crate::BufferedChunk {
                seq: 2,
                chunk: talos_worker::encode::IvfChunk::DisplayDelta(vec![
                    DISPLAY_RECORD_FRAME_BEGIN,
                ]),
            },
            crate::BufferedChunk {
                seq: 3,
                chunk: talos_worker::encode::IvfChunk::DisplayDelta(vec![0x02]),
            },
        ];

        let safe = frame_safe_snapshot(snapshot);

        assert_eq!(safe.len(), 1);
        assert!(matches!(
            safe.first().map(|item| &item.chunk),
            Some(talos_worker::encode::IvfChunk::Metadata(_))
        ));
    }

    #[test]
    fn buffered_display_frame_from_skips_partial_atx2_frame() {
        let pipeline = CapturePipeline::new();
        pipeline.push_chunk(talos_worker::encode::IvfChunk::DisplayDelta(vec![0x02]));
        pipeline.push_chunk(talos_worker::encode::IvfChunk::DisplayDelta(vec![
            DISPLAY_RECORD_FRAME_END,
        ]));
        pipeline.push_chunk(talos_worker::encode::IvfChunk::DisplayDelta(vec![
            DISPLAY_RECORD_FRAME_BEGIN,
        ]));
        pipeline.push_chunk(talos_worker::encode::IvfChunk::DisplayDelta(vec![0x02]));
        pipeline.push_chunk(talos_worker::encode::IvfChunk::DisplayDelta(vec![
            DISPLAY_RECORD_FRAME_END,
        ]));

        let frame = buffered_display_frame_from(&pipeline, 0).expect("complete frame");

        assert_eq!(frame.len(), 3);
        assert_eq!(frame.first().map(|item| item.seq), Some(2));
        assert_eq!(frame.last().map(|item| item.seq), Some(4));
    }

    #[test]
    fn buffered_display_frame_from_requires_complete_atx2_frame() {
        let pipeline = CapturePipeline::new();
        pipeline.push_chunk(talos_worker::encode::IvfChunk::DisplayDelta(vec![
            DISPLAY_RECORD_FRAME_BEGIN,
        ]));
        pipeline.push_chunk(talos_worker::encode::IvfChunk::DisplayDelta(vec![0x02]));

        assert!(buffered_display_frame_from(&pipeline, 0).is_none());
    }
}
