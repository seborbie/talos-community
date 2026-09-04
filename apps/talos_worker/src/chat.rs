//! Bidirectional RMM chat bridge (viewer QUIC/relay ↔ localhost TCP ↔ `talos_worker_chat`).

#[cfg(any(target_os = "windows", target_os = "macos"))]
#[path = "chat_launch.rs"]
pub(crate) mod chat_launch;

#[derive(Clone, Debug)]
pub struct ChatTunnelMeta {
    pub viewer_token: String,
    pub parent_desktop_session_id: Option<String>,
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
mod shared {
    use std::collections::{HashMap, HashSet};
    use std::sync::Arc;
    use std::time::Duration;

    use anyhow::{anyhow, Context, Result};
    use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
    use base64::Engine as _;
    use chacha20poly1305::ChaCha20Poly1305;
    use quinn::{RecvStream, SendStream};
    use rustls::pki_types::ServerName;
    use std::net::UdpSocket as StdUdpSocket;
    use talos_protocol::relay_transport::{
        build_e2e_cipher, build_relay_client_tls_config, parse_relay_target, read_e2e_frame_from,
        read_http_response, write_e2e_frame,
    };
    use talos_protocol::{
        build_chat_frame, ChatWireErrorPayload, ChatWirePayload, OperationErrorCode,
        WorkerChatControlPayload, CHAT_MAX_PAYLOAD_LEN, CHAT_MSG_AUTH, CHAT_MSG_CONTROL,
        CHAT_MSG_ERROR, CHAT_MSG_TEXT, HEARTBEAT_PAYLOAD,
    };
    use tokio::io::{split, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};
    use tokio::sync::RwLock;
    use tokio::time::timeout;
    use tokio_rustls::TlsConnector;
    use tracing::{debug, info, trace, warn};

    const CHAT_LOG_TARGET: &str = "rmm_chat";
    const NO_INTERACTIVE_USER_APPROVAL_MESSAGE: &str = "Endpoint approval could not be requested because no user is currently logged in on this device. Ask someone to sign in, then retry.";

    use super::chat_launch;
    use super::ChatTunnelMeta;

    async fn read_chat_quic_frame(recv: &mut RecvStream) -> Result<Option<(u8, Vec<u8>)>> {
        let mut hdr = [0u8; 3];
        match recv.read_exact(&mut hdr).await {
            Ok(()) => {}
            Err(err) => {
                let message = err.to_string();
                if message.contains("finished early")
                    || message.contains("closed")
                    || message.contains("reset")
                {
                    return Ok(None);
                }
                return Err(anyhow!("read chat header: {message}"));
            }
        }
        let len = u16::from_be_bytes([hdr[1], hdr[2]]) as usize;
        if len > CHAT_MAX_PAYLOAD_LEN {
            return Err(anyhow!("chat payload too large"));
        }
        let mut payload = vec![0u8; len];
        if len > 0 {
            recv.read_exact(&mut payload)
                .await
                .map_err(|e| anyhow!("read chat payload: {e}"))?;
        }
        Ok(Some((hdr[0], payload)))
    }

    async fn write_chat_quic_frame(
        send: &mut SendStream,
        message_type: u8,
        payload: &[u8],
    ) -> Result<()> {
        let frame = build_chat_frame(message_type, payload).map_err(|e| anyhow!("{e}"))?;
        send.write_all(&frame)
            .await
            .map_err(|e| anyhow!("write chat frame: {e}"))?;
        Ok(())
    }

    #[cfg(target_os = "windows")]
    fn chat_exe_path() -> std::path::PathBuf {
        let dir = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.to_path_buf()));
        if let Some(dir) = dir {
            let preferred = dir.join("talos_worker_chat.exe");
            if preferred.exists() {
                return preferred;
            }
            return dir.join("talos_worker_chat.exe");
        }
        std::path::PathBuf::from("talos_worker_chat.exe")
    }

    #[cfg(target_os = "macos")]
    fn chat_exe_path() -> std::path::PathBuf {
        std::path::PathBuf::from(
            "/Library/Talos/Worker/Talos Worker Chat.app/Contents/MacOS/talos_worker_chat",
        )
    }

    #[cfg(target_os = "windows")]
    async fn resolve_chat_launch_target(
        parent_desktop_session_id: Option<&str>,
        helper_target_sessions: &Arc<RwLock<HashMap<String, u32>>>,
    ) -> u32 {
        if let Some(pid) = parent_desktop_session_id {
            if let Some(sid) = helper_target_sessions.read().await.get(pid).copied() {
                return sid;
            }
        }
        unsafe { winapi::um::winbase::WTSGetActiveConsoleSessionId() }
    }

    #[cfg(target_os = "macos")]
    async fn resolve_chat_launch_target(
        _parent_desktop_session_id: Option<&str>,
        _helper_target_sessions: &Arc<RwLock<HashMap<String, u32>>>,
    ) -> u32 {
        0
    }

    async fn read_chat_tcp_frame<R: tokio::io::AsyncRead + Unpin>(
        read: &mut R,
    ) -> Result<Option<(u8, Vec<u8>)>> {
        let mut hdr = [0u8; 3];
        if let Err(err) = tokio::io::AsyncReadExt::read_exact(read, &mut hdr).await {
            let message = err.to_string();
            if message.contains("unexpected end") || message.contains("early eof") {
                return Ok(None);
            }
            return Err(anyhow!("tcp chat header: {message}"));
        }
        let len = u16::from_be_bytes([hdr[1], hdr[2]]) as usize;
        if len > CHAT_MAX_PAYLOAD_LEN {
            return Err(anyhow!("tcp chat payload too large"));
        }
        let mut payload = vec![0u8; len];
        if len > 0 {
            tokio::io::AsyncReadExt::read_exact(read, &mut payload)
                .await
                .map_err(|e| anyhow!("tcp chat payload: {e}"))?;
        }
        Ok(Some((hdr[0], payload)))
    }

    async fn write_chat_tcp_frame<W: tokio::io::AsyncWrite + Unpin>(
        write: &mut W,
        message_type: u8,
        payload: &[u8],
    ) -> Result<()> {
        let frame = build_chat_frame(message_type, payload).map_err(|e| anyhow!("{e}"))?;
        tokio::io::AsyncWriteExt::write_all(write, &frame)
            .await
            .map_err(|e| anyhow!("tcp write chat: {e}"))?;
        Ok(())
    }

    fn parse_chat_frame_owned(bytes: &[u8]) -> Result<(u8, Vec<u8>)> {
        let (t, p) = talos_protocol::parse_chat_frame(bytes).map_err(|e| anyhow!("{e}"))?;
        Ok((t, p.to_vec()))
    }

    fn chat_bridge_error_payload(error: &anyhow::Error) -> Vec<u8> {
        let (code, message, retryable) = if chat_launch::is_no_interactive_user_error(error) {
            (
                OperationErrorCode::NoInteractiveUser,
                NO_INTERACTIVE_USER_APPROVAL_MESSAGE,
                true,
            )
        } else {
            (
                OperationErrorCode::Internal,
                "Unable to launch the endpoint chat approval UI.",
                false,
            )
        };
        serde_json::to_vec(&ChatWireErrorPayload {
            code,
            message: message.to_string(),
            retryable,
        })
        .unwrap_or_else(|_| message.as_bytes().to_vec())
    }

    async fn ensure_tcp_bridge(
        ui_launched: &mut bool,
        listener: &mut Option<TcpListener>,
        bridge_secret: &mut Option<String>,
        meta: &ChatTunnelMeta,
        helper_target_sessions: &Arc<RwLock<HashMap<String, u32>>>,
        session_id: &str,
        ty: u8,
        body: &[u8],
    ) -> Result<()> {
        if *ui_launched {
            trace!(
                target: CHAT_LOG_TARGET,
                session_id = %session_id,
                frame_type = ty,
                body_len = body.len(),
                "chat UI already launched; bridge pending/active"
            );
            return Ok(());
        }
        if ty != CHAT_MSG_TEXT && ty != CHAT_MSG_CONTROL {
            trace!(
                target: CHAT_LOG_TARGET,
                session_id = %session_id,
                frame_type = ty,
                body_len = body.len(),
                "chat bridge ignoring non-text frame before UI launch"
            );
            return Ok(());
        }
        let mut ai_approval_launch: Option<chat_launch::AiApprovalLaunchConfig> = None;
        let launch = if ty == CHAT_MSG_TEXT {
            let payload: ChatWirePayload =
                serde_json::from_slice(body).unwrap_or(ChatWirePayload::Message {
                    id: String::new(),
                    from_viewer: true,
                    text: String::new(),
                    ts_unix_ms: None,
                });
            matches!(
                &payload,
                ChatWirePayload::Message {
                    from_viewer: true,
                    ..
                }
            )
        } else {
            match serde_json::from_slice::<WorkerChatControlPayload>(body) {
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
                    ai_approval_launch = Some(chat_launch::AiApprovalLaunchConfig {
                        approval_id,
                        requester_label,
                        requester_email,
                        organization_name,
                        device_label,
                        reason,
                        expires_at_unix_ms,
                        approval_window_expires_at_unix_ms,
                    });
                    true
                }
                _ => false,
            }
        };
        if !launch {
            trace!(
                target: CHAT_LOG_TARGET,
                session_id = %session_id,
                frame_type = ty,
                "chat bridge ignoring non-launch frame before UI launch"
            );
            return Ok(());
        }
        debug!(
            target: CHAT_LOG_TARGET,
            session_id = %session_id,
            frame_type = ty,
            body_len = body.len(),
            "chat bridge launching UI for first viewer frame"
        );
        let secret = uuid::Uuid::new_v4().to_string();
        let lis = TcpListener::bind("127.0.0.1:0")
            .await
            .context("bind chat localhost bridge")?;
        let port = lis.local_addr()?.port();
        let target_session = resolve_chat_launch_target(
            meta.parent_desktop_session_id.as_deref(),
            helper_target_sessions,
        )
        .await;
        let exe = chat_exe_path();
        debug!(
            target: CHAT_LOG_TARGET,
            session_id = %session_id,
            target_session,
            exe = %exe.display(),
            port,
            "chat bridge launching agent chat process"
        );
        if let Some(config) = ai_approval_launch.as_ref() {
            chat_launch::launch_ai_approval_ui(target_session, &exe, port, &secret, config)
                .with_context(|| format!("launch {} for AI approval", exe.display()))?;
        } else {
            chat_launch::launch_chat_ui(target_session, &exe, port, &secret)
                .with_context(|| format!("launch {}", exe.display()))?;
        }
        *bridge_secret = Some(secret);
        *listener = Some(lis);
        *ui_launched = true;
        info!(target: CHAT_LOG_TARGET, session_id = %session_id, port, "chat UI launched; waiting for local TCP");
        Ok(())
    }

    async fn accept_tcp_handshake(
        listener: &TcpListener,
        bridge_secret: &str,
    ) -> Result<TcpStream> {
        debug!(
            target: CHAT_LOG_TARGET,
            "chat bridge waiting for local TCP handshake"
        );
        let (mut incoming, _) = timeout(Duration::from_secs(45), listener.accept())
            .await
            .map_err(|_| anyhow!("timed out waiting for chat UI tcp"))?
            .map_err(|e| anyhow!("tcp accept: {e}"))?;
        let hello = read_chat_tcp_frame(&mut incoming).await?;
        let Some((hty, hbody)) = hello else {
            return Err(anyhow!("chat tcp closed before handshake"));
        };
        if hty != CHAT_MSG_AUTH || String::from_utf8_lossy(&hbody).trim() != bridge_secret {
            warn!(
                target: CHAT_LOG_TARGET,
                frame_type = hty,
                body_len = hbody.len(),
                "chat tcp handshake secret mismatch"
            );
            return Err(anyhow!("chat tcp handshake secret mismatch"));
        }
        debug!(
            target: CHAT_LOG_TARGET,
            "chat bridge local TCP handshake accepted"
        );
        Ok(incoming)
    }

    pub async fn bridge_chat_quic_streams(
        mut send: SendStream,
        mut recv: RecvStream,
        session_id: String,
        tunnels: Arc<RwLock<HashMap<String, ChatTunnelMeta>>>,
        helper_target_sessions: Arc<RwLock<HashMap<String, u32>>>,
    ) -> Result<()> {
        let meta = {
            let g = tunnels.read().await;
            g.get(&session_id)
                .cloned()
                .ok_or_else(|| anyhow!("missing chat tunnel registration"))?
        };
        debug!(
            target: CHAT_LOG_TARGET,
            session_id = %session_id,
            has_parent_desktop_session_id = meta.parent_desktop_session_id.is_some(),
            "chat quic bridge starting"
        );

        let auth = read_chat_quic_frame(&mut recv)
            .await?
            .ok_or_else(|| anyhow!("chat closed before auth"))?;
        if auth.0 != CHAT_MSG_AUTH || auth.1 != meta.viewer_token.as_bytes() {
            let _ = write_chat_quic_frame(
                &mut send,
                talos_protocol::CHAT_MSG_ERROR,
                b"invalid chat auth token",
            )
            .await;
            return Err(anyhow!("invalid chat auth"));
        }
        debug!(
            target: CHAT_LOG_TARGET,
            session_id = %session_id,
            "chat quic auth accepted"
        );

        let mut ui_launched = false;
        let mut bridge_secret: Option<String> = None;
        let mut listener: Option<TcpListener> = None;

        loop {
            let (ty, body) = match read_chat_quic_frame(&mut recv).await? {
                Some(f) => f,
                None => {
                    debug!(
                        target: CHAT_LOG_TARGET,
                        session_id = %session_id,
                        "chat quic viewer stream closed before/after UI launch"
                    );
                    break;
                }
            };
            trace!(
                target: CHAT_LOG_TARGET,
                session_id = %session_id,
                frame_type = ty,
                body_len = body.len(),
                "chat quic frame received from viewer"
            );

            if let Err(error) = ensure_tcp_bridge(
                &mut ui_launched,
                &mut listener,
                &mut bridge_secret,
                &meta,
                &helper_target_sessions,
                &session_id,
                ty,
                &body,
            )
            .await
            {
                let payload = chat_bridge_error_payload(&error);
                if let Err(send_error) =
                    write_chat_quic_frame(&mut send, CHAT_MSG_ERROR, &payload).await
                {
                    warn!(
                        target: CHAT_LOG_TARGET,
                        session_id = %session_id,
                        error = %send_error,
                        "chat bridge failed to send UI launch error to viewer"
                    );
                }
                return Err(error);
            }

            if ui_launched && listener.is_some() {
                let lis = listener.take().unwrap();
                let secret = bridge_secret.clone().unwrap();
                let mut tcp = accept_tcp_handshake(&lis, &secret).await?;
                debug!(
                    target: CHAT_LOG_TARGET,
                    session_id = %session_id,
                    frame_type = ty,
                    body_len = body.len(),
                    "chat bridge forwarding first viewer frame to UI"
                );
                write_chat_tcp_frame(&mut tcp, ty, &body).await?;
                let (mut tcp_read, mut tcp_write) = tcp.into_split();
                'tcp_bridge: loop {
                    tokio::select! {
                        q = read_chat_quic_frame(&mut recv) => {
                            match q {
                                Ok(Some(inner)) => {
                                    trace!(
                                        target: CHAT_LOG_TARGET,
                                        session_id = %session_id,
                                        frame_type = inner.0,
                                        body_len = inner.1.len(),
                                        "chat quic viewer frame forwarding to UI"
                                    );
                                    if let Err(err) = write_chat_tcp_frame(&mut tcp_write, inner.0, &inner.1).await {
                                        warn!(target: CHAT_LOG_TARGET, session_id = %session_id, error = %err, "chat UI tcp write failed; waiting for relaunch");
                                        break 'tcp_bridge;
                                    }
                                }
                                Ok(None) => {
                                    debug!(target: CHAT_LOG_TARGET, session_id = %session_id, "chat quic viewer stream closed");
                                    return Ok(());
                                }
                                Err(e) => return Err(e),
                            }
                        }
                        t = read_chat_tcp_frame(&mut tcp_read) => {
                            match t {
                                Ok(Some(inner)) => {
                                    trace!(
                                        target: CHAT_LOG_TARGET,
                                        session_id = %session_id,
                                        frame_type = inner.0,
                                        body_len = inner.1.len(),
                                        "chat UI frame forwarding to quic viewer"
                                    );
                                    write_chat_quic_frame(&mut send, inner.0, &inner.1).await?;
                                }
                                Ok(None) => {
                                    warn!(target: CHAT_LOG_TARGET, session_id = %session_id, "chat UI tcp closed; waiting for relaunch");
                                    break 'tcp_bridge;
                                }
                                Err(e) => {
                                    warn!(target: CHAT_LOG_TARGET, session_id = %session_id, error = %e, "chat UI tcp read failed; waiting for relaunch");
                                    break 'tcp_bridge;
                                }
                            }
                        }
                    }
                }
                ui_launched = false;
                bridge_secret = None;
                listener = None;
            }
        }
        Ok(())
    }

    pub async fn accept_chat_quic_connections(
        endpoint: quinn::Endpoint,
        local_addrs: Vec<talos_protocol::LocalAddr>,
        session_id: String,
        tunnels: Arc<RwLock<HashMap<String, ChatTunnelMeta>>>,
        helper_target_sessions: Arc<RwLock<HashMap<String, u32>>>,
    ) -> Result<()> {
        loop {
            let Some(connecting) = endpoint.accept().await else {
                break;
            };
            let connection = match connecting.await {
                Ok(conn) => conn,
                Err(err) => {
                    warn!(target: CHAT_LOG_TARGET, error = %err, "chat quic connection failed");
                    continue;
                }
            };

            let sid = session_id.clone();
            let tunnels_cl = tunnels.clone();
            let helpers_cl = helper_target_sessions.clone();
            let la = local_addrs.clone();
            let remote = connection.remote_address();
            let source = if crate::is_lan_connection(remote, &la) {
                "lan"
            } else {
                "reflex"
            };
            info!(target: CHAT_LOG_TARGET, session_id = %sid, %remote, source, "chat quic connection accepted");

            tokio::spawn(async move {
                loop {
                    let stream = connection.accept_bi().await;
                    let (send, recv) = match stream {
                        Ok(s) => s,
                        Err(err) => {
                            if err.to_string().contains("closed") {
                                break;
                            }
                            warn!(target: CHAT_LOG_TARGET, error = %err, "chat bi stream accept failed");
                            break;
                        }
                    };
                    let sid2 = sid.clone();
                    let tunnels_c2 = tunnels_cl.clone();
                    let helpers_c2 = helpers_cl.clone();
                    tokio::spawn(async move {
                        if let Err(err) =
                            bridge_chat_quic_streams(send, recv, sid2, tunnels_c2, helpers_c2).await
                        {
                            warn!(target: CHAT_LOG_TARGET, error = %err, "chat quic bridge ended");
                        }
                    });
                }
            });
        }
        Ok(())
    }

    pub async fn handle_chat_tunnel_prepare(
        payload: &talos_protocol::TunnelPreparePayload,
        write: &mut crate::WsSink,
        punch_sockets: &Arc<RwLock<HashMap<String, Arc<StdUdpSocket>>>>,
        chat_relay_sessions: &Arc<RwLock<HashSet<String>>>,
        chat_tunnels: &Arc<RwLock<HashMap<String, ChatTunnelMeta>>>,
        helper_target_sessions: Arc<RwLock<HashMap<String, u32>>>,
    ) -> Result<()> {
        let viewer_token = payload
            .viewer_session_token
            .clone()
            .ok_or_else(|| anyhow!("chat tunnel_prepare missing viewer_session_token"))?;

        {
            let mut g = chat_tunnels.write().await;
            g.insert(
                payload.session_id.clone(),
                ChatTunnelMeta {
                    viewer_token,
                    parent_desktop_session_id: payload.parent_desktop_session_id.clone(),
                },
            );
        }

        info!(target: CHAT_LOG_TARGET, session_id = %payload.session_id, mode = "chat", "tunnel_prepare received");

        if let (Some(relay_url), Some(e2e_key)) =
            (payload.relay_url.clone(), payload.e2e_key.clone())
        {
            start_chat_relay_client_once(
                payload.session_id.clone(),
                relay_url,
                e2e_key,
                chat_relay_sessions.clone(),
                chat_tunnels.clone(),
                helper_target_sessions.clone(),
            )
            .await;
        }

        let (endpoint, local_addr, punch_socket, stun_result) =
            crate::build_quic_endpoint(&payload.psk_cert_pem, &payload.psk_key_pem).await?;

        info!(
            target: CHAT_LOG_TARGET,
            session_id = %payload.session_id,
            local_addr = %local_addr,
            "chat quic server bound"
        );

        match stun_result {
            Ok(reflex) => {
                let response = talos_protocol::QuicReflexPayload {
                    session_id: payload.session_id.clone(),
                    reflex: reflex.clone(),
                };
                crate::send_message(write, "quic_reflex", response).await?;
                info!(target: CHAT_LOG_TARGET, session_id = %payload.session_id, "chat quic_reflex sent");
            }
            Err(err) => {
                warn!(
                    session_id = %payload.session_id,
                    error = %err,
                    "chat stun failed; continuing without reflex"
                );
            }
        }

        {
            let mut sockets = punch_sockets.write().await;
            sockets.insert(payload.session_id.clone(), Arc::new(punch_socket));
        }

        let addrs = crate::local_addrs();
        let session_id = payload.session_id.clone();
        let tunnels = chat_tunnels.clone();
        let helpers = helper_target_sessions.clone();
        tokio::spawn(async move {
            if let Err(err) =
                accept_chat_quic_connections(endpoint, addrs, session_id.clone(), tunnels, helpers)
                    .await
            {
                warn!(
                    target: CHAT_LOG_TARGET,
                    session_id = %session_id,
                    error = %err,
                    "chat quic accept loop ended"
                );
            }
        });

        Ok(())
    }

    async fn start_chat_relay_client_once(
        session_id: String,
        relay_url: String,
        e2e_key: String,
        relay_sessions: Arc<RwLock<HashSet<String>>>,
        chat_tunnels: Arc<RwLock<HashMap<String, ChatTunnelMeta>>>,
        helper_target_sessions: Arc<RwLock<HashMap<String, u32>>>,
    ) {
        {
            let mut sessions = relay_sessions.write().await;
            if sessions.contains(&session_id) {
                debug!(
                    target: CHAT_LOG_TARGET,
                    session_id = %session_id,
                    "chat relay client already running"
                );
                return;
            }
            sessions.insert(session_id.clone());
        }

        tokio::spawn(async move {
            if let Err(err) = run_chat_relay_client(
                session_id.clone(),
                relay_url,
                e2e_key,
                chat_tunnels,
                helper_target_sessions,
            )
            .await
            {
                warn!(
                    target: CHAT_LOG_TARGET,
                    session_id = %session_id,
                    error = %err,
                    "chat relay client ended unexpectedly"
                );
            }
            let mut sessions = relay_sessions.write().await;
            sessions.remove(&session_id);
        });
    }

    async fn run_chat_relay_client(
        session_id: String,
        relay_url: String,
        e2e_key_b64: String,
        chat_tunnels: Arc<RwLock<HashMap<String, ChatTunnelMeta>>>,
        helper_target_sessions: Arc<RwLock<HashMap<String, u32>>>,
    ) -> Result<()> {
        let relay_target = parse_relay_target(&relay_url)?;
        let addr = format!("{}:{}", relay_target.host, relay_target.port);
        let connect_timeout = Duration::from_secs(10);
        debug!(
            target: CHAT_LOG_TARGET,
            session_id = %session_id,
            relay_addr = %addr,
            "chat relay client connecting tcp"
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
        debug!(
            target: CHAT_LOG_TARGET,
            session_id = %session_id,
            relay_host = %relay_target.host,
            "chat relay client tls connected"
        );

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
        let cipher: ChaCha20Poly1305 = build_e2e_cipher(&key_bytes)?;

        let mut send_counter = 0u64;
        write_e2e_frame(&mut stream, &cipher, &mut send_counter, b"hello-world").await?;
        info!(target: CHAT_LOG_TARGET, session_id = %session_id, "chat relay hello-world frame sent");

        let (mut reader, mut writer) = split(stream);

        let meta = chat_tunnels
            .read()
            .await
            .get(&session_id)
            .cloned()
            .ok_or_else(|| anyhow!("missing chat tunnel meta for relay"))?;

        let first_payload = loop {
            let payload = read_e2e_frame_from(&mut reader, &cipher).await?;
            if payload == HEARTBEAT_PAYLOAD || payload == b"hello-world" {
                continue;
            }
            break payload;
        };
        debug!(
            target: CHAT_LOG_TARGET,
            session_id = %session_id,
            payload_len = first_payload.len(),
            "chat relay first payload received"
        );

        let (auth_ty, auth_body) = parse_chat_frame_owned(&first_payload)?;
        if auth_ty != CHAT_MSG_AUTH || auth_body != meta.viewer_token.as_bytes() {
            write_e2e_frame(
                &mut writer,
                &cipher,
                &mut send_counter,
                &build_chat_frame(talos_protocol::CHAT_MSG_ERROR, b"invalid chat auth")?,
            )
            .await?;
            return Err(anyhow!("invalid chat relay auth"));
        }
        debug!(
            target: CHAT_LOG_TARGET,
            session_id = %session_id,
            "chat relay auth accepted"
        );

        let mut ui_launched = false;
        let mut bridge_secret: Option<String> = None;
        let mut listener: Option<TcpListener> = None;

        loop {
            let payload = read_e2e_frame_from(&mut reader, &cipher).await?;
            if payload == HEARTBEAT_PAYLOAD {
                continue;
            }
            let (ty, body) = parse_chat_frame_owned(&payload)?;
            trace!(
                target: CHAT_LOG_TARGET,
                session_id = %session_id,
                frame_type = ty,
                body_len = body.len(),
                "chat relay frame received from viewer"
            );

            if let Err(error) = ensure_tcp_bridge(
                &mut ui_launched,
                &mut listener,
                &mut bridge_secret,
                &meta,
                &helper_target_sessions,
                &session_id,
                ty,
                &body,
            )
            .await
            {
                let payload = chat_bridge_error_payload(&error);
                match build_chat_frame(CHAT_MSG_ERROR, &payload) {
                    Ok(frame) => {
                        if let Err(send_error) =
                            write_e2e_frame(&mut writer, &cipher, &mut send_counter, &frame).await
                        {
                            warn!(
                                target: CHAT_LOG_TARGET,
                                session_id = %session_id,
                                error = %send_error,
                                "chat relay failed to send UI launch error to viewer"
                            );
                        }
                    }
                    Err(send_error) => {
                        warn!(
                            target: CHAT_LOG_TARGET,
                            session_id = %session_id,
                            error = %send_error,
                            "chat relay failed to encode UI launch error"
                        );
                    }
                }
                return Err(error);
            }

            if !ui_launched {
                continue;
            }

            let lis = listener
                .take()
                .ok_or_else(|| anyhow!("chat listener missing after launch"))?;
            let secret = bridge_secret.clone().unwrap();
            let mut tcp = accept_tcp_handshake(&lis, &secret).await?;
            debug!(
                target: CHAT_LOG_TARGET,
                session_id = %session_id,
                frame_type = ty,
                body_len = body.len(),
                "chat relay forwarding first viewer frame to UI"
            );
            write_chat_tcp_frame(&mut tcp, ty, &body).await?;
            let (mut tcp_read, mut tcp_write) = tcp.into_split();
            'tcp_bridge: loop {
                tokio::select! {
                    r = read_e2e_frame_from(&mut reader, &cipher) => {
                        let p = r?;
                        if p == HEARTBEAT_PAYLOAD {
                            continue;
                        }
                        let (pt, pb) = parse_chat_frame_owned(&p)?;
                        trace!(
                            target: CHAT_LOG_TARGET,
                            session_id = %session_id,
                            frame_type = pt,
                            body_len = pb.len(),
                            "chat relay viewer frame forwarding to UI"
                        );
                        if let Err(err) = write_chat_tcp_frame(&mut tcp_write, pt, &pb).await {
                            warn!(target: CHAT_LOG_TARGET, session_id = %session_id, error = %err, "chat relay UI tcp write failed; waiting for relaunch");
                            break 'tcp_bridge;
                        }
                    }
                    t = read_chat_tcp_frame(&mut tcp_read) => {
                        match t {
                            Ok(Some((tt, tb))) => {
                                trace!(
                                    target: CHAT_LOG_TARGET,
                                    session_id = %session_id,
                                    frame_type = tt,
                                    body_len = tb.len(),
                                    "chat UI frame forwarding to relay viewer"
                                );
                                let enc = build_chat_frame(tt, &tb)?;
                                write_e2e_frame(&mut writer, &cipher, &mut send_counter, &enc).await?;
                            }
                            Ok(None) => {
                                warn!(target: CHAT_LOG_TARGET, session_id = %session_id, "chat relay UI tcp closed; waiting for relaunch");
                                break 'tcp_bridge;
                            }
                            Err(e) => {
                                warn!(target: CHAT_LOG_TARGET, session_id = %session_id, error = %e, "chat relay UI tcp read failed; waiting for relaunch");
                                break 'tcp_bridge;
                            }
                        }
                    }
                }
            }
            ui_launched = false;
            bridge_secret = None;
            listener = None;
        }
    }

    pub async fn handle_chat_relay_prepare(
        payload: &talos_protocol::RelayPreparePayload,
        chat_relay_sessions: &Arc<RwLock<HashSet<String>>>,
        chat_tunnels: &Arc<RwLock<HashMap<String, ChatTunnelMeta>>>,
        helper_target_sessions: Arc<RwLock<HashMap<String, u32>>>,
    ) {
        info!(
            target: CHAT_LOG_TARGET,
            session_id = %payload.session_id,
            mode = "chat",
            "relay_prepare received"
        );
        start_chat_relay_client_once(
            payload.session_id.clone(),
            payload.relay_url.clone(),
            payload.e2e_key.clone(),
            chat_relay_sessions.clone(),
            chat_tunnels.clone(),
            helper_target_sessions,
        )
        .await;
    }

    pub async fn cleanup_chat_session(
        session_id: &str,
        chat_tunnels: &Arc<RwLock<HashMap<String, ChatTunnelMeta>>>,
    ) {
        chat_tunnels.write().await.remove(session_id);
        info!(target: CHAT_LOG_TARGET, session_id = %session_id, "chat tunnel registration cleared");
    }
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
pub use shared::*;
