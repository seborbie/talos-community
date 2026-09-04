//! Viewer ↔ agent chat transport (QUIC with relay fallback).

use std::net::{SocketAddr, UdpSocket};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context};
use base64::engine::general_purpose::{
    STANDARD as BASE64_STANDARD, URL_SAFE_NO_PAD as BASE64_URL_SAFE_NO_PAD,
};
use base64::Engine as _;
use chacha20poly1305::ChaCha20Poly1305;
use quinn::TokioRuntime;
use quinn::{Connection, Endpoint, EndpointConfig};
use reqwest::Client;
use rustls::pki_types::ServerName;
use serde_json::json;
use talos_protocol::relay_transport::{
    build_e2e_cipher, parse_relay_target, read_e2e_frame_from, read_http_response, write_e2e_frame,
};
use talos_protocol::{
    build_chat_frame, ChatAckPayload, ChatSessionCapabilitiesHttpResponse, ChatWirePayload,
    LocalAddr, ReflexAddress, CHAT_MSG_ACK, CHAT_MSG_AUTH, CHAT_MSG_TEXT, HEARTBEAT_PAYLOAD,
};
use tauri::{Emitter, Window};
use tokio::io::{split, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::{mpsc, Mutex as AsyncMutex};
use tokio::task::JoinHandle;
use tokio::time::{interval, MissedTickBehavior};
use tokio_rustls::TlsConnector;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, trace, warn};

const CHAT_LOG_TARGET: &str = "rmm_chat";

#[derive(Clone, Default)]
pub struct ChatConnectionState(pub Arc<AsyncMutex<Option<ChatIo>>>);

pub struct ChatIo {
    pub(crate) shutdown: CancellationToken,
    pub(crate) outbound_tx: mpsc::UnboundedSender<Vec<u8>>,
    pub(crate) join: JoinHandle<()>,
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

fn emit_window(window: &Window, event: &str, payload: serde_json::Value) {
    if let Err(err) = window.emit_to(window.label(), event, payload) {
        warn!(
            target: CHAT_LOG_TARGET,
            event = event,
            error = %err,
            "failed to emit window-scoped chat event"
        );
    }
}

async fn fetch_chat_capabilities(
    api_base: &str,
    session_id: &str,
    token: &str,
) -> Result<ChatSessionCapabilitiesHttpResponse, anyhow::Error> {
    let url = format!(
        "{}/api/rmm/chat/session/{}/capabilities?token={}",
        api_base.trim_end_matches('/'),
        session_id,
        urlencoding::encode(token)
    );
    debug!(
        target: CHAT_LOG_TARGET,
        session_id = %session_id,
        api_base = %api_base,
        "viewer chat fetching capabilities"
    );
    let response = Client::new()
        .get(url)
        .send()
        .await
        .context("chat capabilities")?;
    if !response.status().is_success() {
        return Err(anyhow!("chat capabilities failed ({})", response.status()));
    }
    let capabilities: ChatSessionCapabilitiesHttpResponse = response.json().await?;
    debug!(
        target: CHAT_LOG_TARGET,
        session_id = %session_id,
        transports = ?capabilities.transports,
        "viewer chat capabilities received"
    );
    Ok(capabilities)
}

async fn post_chat_viewer_connected(
    api_base: &str,
    session_id: &str,
    token: &str,
) -> Result<(), anyhow::Error> {
    let url = format!(
        "{}/api/rmm/chat/session/{}/viewer-connected?token={}",
        api_base.trim_end_matches('/'),
        session_id,
        urlencoding::encode(token)
    );
    trace!(
        target: CHAT_LOG_TARGET,
        session_id = %session_id,
        "viewer chat posting viewer-connected"
    );
    let response = Client::new().post(url).send().await?;
    if !response.status().is_success() {
        return Err(anyhow!(
            "chat viewer-connected failed ({})",
            response.status()
        ));
    }
    Ok(())
}

async fn connect_chat_quic_transport(
    session_id: &str,
    token: &str,
    api_base: &str,
    agent_reflex: &ReflexAddress,
    agent_host: Option<String>,
    agent_local_addrs: Option<Vec<LocalAddr>>,
    psk_cert_pem: &str,
    quic_timeout_ms: Option<u64>,
) -> Result<(Endpoint, Connection), anyhow::Error> {
    let viewer_addrs = crate::viewer_local_addrs();
    let lan_candidate = match &agent_local_addrs {
        Some(addrs) => crate::pick_lan_candidate(&viewer_addrs, addrs),
        None => agent_host.filter(|host| !host.trim().is_empty()),
    };
    let reflex_addr: SocketAddr = format!("{}:{}", agent_reflex.ip, agent_reflex.port)
        .parse()
        .context("parse agent reflex address")?;
    let lan_addr = lan_candidate
        .as_ref()
        .map(|ip| format!("{}:{}", ip, agent_reflex.port).parse::<SocketAddr>())
        .transpose()
        .context("parse lan candidate")?;

    debug!(
        target: CHAT_LOG_TARGET,
        session_id = %session_id,
        viewer_addrs = ?viewer_addrs,
        agent_reflex = %reflex_addr,
        lan_addr = ?lan_addr,
        quic_timeout_ms = quic_timeout_ms.unwrap_or(500),
        "viewer chat preparing quic transport"
    );

    let socket = UdpSocket::bind("0.0.0.0:0").context("bind quic socket")?;
    socket
        .set_nonblocking(true)
        .context("set quic socket nonblocking")?;

    let viewer_reflex = tokio::task::spawn_blocking({
        let stun_socket = socket.try_clone().ok();
        move || -> Result<SocketAddr, anyhow::Error> {
            let stun_socket = stun_socket.ok_or_else(|| anyhow!("stun socket clone failed"))?;
            crate::query_configured_stun_reflex(stun_socket)
        }
    })
    .await
    .context("join stun task")??;

    debug!(
        target: CHAT_LOG_TARGET,
        session_id = %session_id,
        viewer_reflex = %viewer_reflex,
        "viewer chat stun reflex resolved"
    );

    let reflex_url = format!(
        "{}/api/rmm/chat/session/{}/viewer-reflex?token={}",
        api_base.trim_end_matches('/'),
        session_id,
        urlencoding::encode(token)
    );
    let reflex_body = serde_json::json!({
        "ip": viewer_reflex.ip().to_string(),
        "port": viewer_reflex.port(),
    });
    let reflex_response = Client::new()
        .post(reflex_url)
        .json(&reflex_body)
        .send()
        .await
        .context("post chat viewer reflex")?;
    if !reflex_response.status().is_success() {
        return Err(anyhow!(
            "chat viewer reflex failed ({})",
            reflex_response.status()
        ));
    }

    trace!(
        target: CHAT_LOG_TARGET,
        session_id = %session_id,
        "viewer chat reflex posted"
    );

    let mut endpoint = Endpoint::new(
        EndpointConfig::default(),
        None,
        socket,
        Arc::new(TokioRuntime),
    )
    .context("create quic endpoint")?;
    let client_config = crate::build_client_config(psk_cert_pem)?;
    endpoint.set_default_client_config(client_config);

    let quic_timeout = Duration::from_millis(quic_timeout_ms.unwrap_or(500));
    let connection = if let Some(lan_addr) = lan_addr {
        let mut lan_handle = tokio::spawn(crate::run_quic_with_timeout(
            endpoint.clone(),
            session_id.to_string(),
            lan_addr,
            quic_timeout,
        ));
        let mut reflex_handle = tokio::spawn(crate::run_quic_with_timeout(
            endpoint.clone(),
            session_id.to_string(),
            reflex_addr,
            quic_timeout,
        ));

        let mut errors: Vec<anyhow::Error> = Vec::new();
        loop {
            tokio::select! {
                result = &mut lan_handle => {
                    match result {
                        Ok(Ok(connection)) => {
                            debug!(
                                target: CHAT_LOG_TARGET,
                                session_id = %session_id,
                                target = %lan_addr,
                                "viewer chat quic lan candidate connected"
                            );
                            reflex_handle.abort();
                            break connection;
                        }
                        Ok(Err(error)) => {
                            debug!(
                                target: CHAT_LOG_TARGET,
                                session_id = %session_id,
                                target = %lan_addr,
                                error = %error,
                                "viewer chat quic lan candidate failed"
                            );
                            errors.push(error);
                        }
                        Err(error) => {
                            errors.push(anyhow!("lan connect task: {error}"));
                        }
                    }
                }
                result = &mut reflex_handle => {
                    match result {
                        Ok(Ok(connection)) => {
                            debug!(
                                target: CHAT_LOG_TARGET,
                                session_id = %session_id,
                                target = %reflex_addr,
                                "viewer chat quic reflex candidate connected"
                            );
                            lan_handle.abort();
                            break connection;
                        }
                        Ok(Err(error)) => {
                            debug!(
                                target: CHAT_LOG_TARGET,
                                session_id = %session_id,
                                target = %reflex_addr,
                                error = %error,
                                "viewer chat quic reflex candidate failed"
                            );
                            errors.push(error);
                        }
                        Err(error) => {
                            errors.push(anyhow!("reflex connect task: {error}"));
                        }
                    }
                }
            }

            if lan_handle.is_finished() && reflex_handle.is_finished() {
                return Err(errors
                    .pop()
                    .unwrap_or_else(|| anyhow!("quic connect failed")));
            }
        }
    } else {
        debug!(
            target: CHAT_LOG_TARGET,
            session_id = %session_id,
            target = %reflex_addr,
            "viewer chat attempting reflex-only quic"
        );
        crate::run_quic_with_timeout(
            endpoint.clone(),
            session_id.to_string(),
            reflex_addr,
            quic_timeout,
        )
        .await?
    };

    Ok((endpoint, connection))
}

async fn request_chat_relay(
    api_base: &str,
    session_id: &str,
    token: &str,
) -> Result<(), anyhow::Error> {
    let url = format!(
        "{}/api/rmm/chat/session/{}/request-relay?token={}",
        api_base.trim_end_matches('/'),
        session_id,
        urlencoding::encode(token)
    );
    debug!(
        target: CHAT_LOG_TARGET,
        session_id = %session_id,
        "viewer chat requesting relay"
    );
    let response = Client::new().post(url).send().await?;
    if !response.status().is_success() {
        return Err(anyhow!("chat request relay failed ({})", response.status()));
    }
    Ok(())
}

struct ChatRelayPipe {
    reader: tokio::io::ReadHalf<tokio_rustls::client::TlsStream<TcpStream>>,
    writer: tokio::io::WriteHalf<tokio_rustls::client::TlsStream<TcpStream>>,
    cipher: ChaCha20Poly1305,
    send_counter: u64,
}

async fn connect_chat_relay_transport(
    session_id: &str,
    relay_url: &str,
    e2e_key: &str,
) -> Result<ChatRelayPipe, anyhow::Error> {
    let relay_target = parse_relay_target(relay_url)?;
    let addr = format!("{}:{}", relay_target.host, relay_target.port);
    let connect_timeout = crate::relay_connect_timeout();
    debug!(
        target: CHAT_LOG_TARGET,
        session_id = %session_id,
        relay_addr = %addr,
        "viewer chat connecting relay tcp"
    );
    let tcp_stream = tokio::time::timeout(connect_timeout, TcpStream::connect(addr.clone()))
        .await
        .map_err(|_| anyhow!("connect relay tcp timed out"))?
        .context("connect relay tcp")?;
    tcp_stream
        .set_nodelay(true)
        .context("set relay tcp_nodelay")?;

    let tls_config = crate::build_tls_config()?;
    let connector = TlsConnector::from(Arc::new(tls_config));
    let server_name =
        ServerName::try_from(relay_target.host.clone()).context("build relay server name")?;
    let mut stream =
        tokio::time::timeout(connect_timeout, connector.connect(server_name, tcp_stream))
            .await
            .map_err(|_| anyhow!("relay tls connect timed out"))?
            .context("relay tls connect")?;
    debug!(
        target: CHAT_LOG_TARGET,
        session_id = %session_id,
        relay_host = %relay_target.host,
        "viewer chat relay tls connected"
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
    tokio::time::timeout(connect_timeout, read_http_response(&mut stream))
        .await
        .map_err(|_| anyhow!("read relay response timed out"))??;

    let key_bytes = BASE64_URL_SAFE_NO_PAD
        .decode(e2e_key.trim())
        .or_else(|_| BASE64_STANDARD.decode(e2e_key.trim()))
        .context("decode relay e2e key")?;
    let cipher = build_e2e_cipher(&key_bytes)?;

    let mut send_counter = 0u64;
    write_e2e_frame(&mut stream, &cipher, &mut send_counter, b"hello-world")
        .await
        .context("send relay hello")?;
    debug!(
        target: CHAT_LOG_TARGET,
        session_id = %session_id,
        "viewer chat relay hello sent"
    );

    let (reader, writer) = split(stream);
    Ok(ChatRelayPipe {
        reader,
        writer,
        cipher,
        send_counter,
    })
}

async fn open_chat_relay_session_pipe(
    api_base: &str,
    session_id: &str,
    token: &str,
    relay_url: &Option<String>,
    e2e_key: &Option<String>,
) -> Result<ChatRelayPipe, String> {
    let relay_url = relay_url
        .as_ref()
        .ok_or_else(|| "relay url missing".to_string())?;
    let e2e_key = e2e_key
        .as_ref()
        .ok_or_else(|| "relay e2e key missing".to_string())?;
    request_chat_relay(api_base, session_id, token)
        .await
        .map_err(|e| e.to_string())?;
    connect_chat_relay_transport(session_id, relay_url, e2e_key)
        .await
        .map_err(|e| e.to_string())
}

async fn read_chat_quic_frame(
    recv: &mut quinn::RecvStream,
) -> Result<Option<(u8, Vec<u8>)>, anyhow::Error> {
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
    if len > talos_protocol::CHAT_MAX_PAYLOAD_LEN {
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

async fn run_chat_quic_session(
    window: Window,
    connection: Connection,
    mut outbound: mpsc::UnboundedReceiver<Vec<u8>>,
    shutdown: CancellationToken,
    session_id: String,
    token: String,
    api_base: String,
) -> Result<(), anyhow::Error> {
    let (mut send, mut recv) = connection.open_bi().await.context("open chat bi stream")?;
    debug!(
        target: CHAT_LOG_TARGET,
        session_id = %session_id,
        "viewer chat quic bi stream opened"
    );

    let auth = build_chat_frame(CHAT_MSG_AUTH, token.as_bytes())?;
    send.write_all(&auth).await?;
    trace!(
        target: CHAT_LOG_TARGET,
        session_id = %session_id,
        "viewer chat quic auth frame sent"
    );

    post_chat_viewer_connected(&api_base, &session_id, &token).await?;
    debug!(
        target: CHAT_LOG_TARGET,
        session_id = %session_id,
        "viewer chat quic session marked viewer-connected"
    );

    let mut hb = interval(Duration::from_secs(2));
    hb.set_missed_tick_behavior(MissedTickBehavior::Delay);

    loop {
        tokio::select! {
            biased;
            _ = shutdown.cancelled() => break,
            _ = hb.tick() => {
                let url = format!(
                    "{}/api/rmm/chat/session/{}/viewer-heartbeat?token={}",
                    api_base.trim_end_matches('/'),
                    session_id,
                    urlencoding::encode(&token)
                );
                let _ = Client::new().post(url).send().await;
            }
            m = outbound.recv() => {
                let Some(bytes) = m else { break };
                trace!(
                    target: CHAT_LOG_TARGET,
                    session_id = %session_id,
                    frame_len = bytes.len(),
                    "viewer chat quic outbound frame"
                );
                send.write_all(&bytes).await?;
            }
            f = read_chat_quic_frame(&mut recv) => {
                match f? {
                    Some((ty, body)) if ty == CHAT_MSG_TEXT => {
                        trace!(
                            target: CHAT_LOG_TARGET,
                            session_id = %session_id,
                            body_len = body.len(),
                            "viewer chat quic inbound text frame"
                        );
                        if let Ok(ChatWirePayload::Message {
                            id,
                            from_viewer,
                            text,
                            ts_unix_ms,
                        }) = serde_json::from_slice(&body)
                        {
                            let ack = serde_json::to_vec(&ChatAckPayload {
                                message_id: id.clone(),
                            })?;
                            let ack_frame = build_chat_frame(CHAT_MSG_ACK, &ack)?;
                            send.write_all(&ack_frame).await?;
                            debug!(
                                target: CHAT_LOG_TARGET,
                                session_id = %session_id,
                                message_id = %id,
                                from_viewer,
                                text_len = text.len(),
                                "viewer chat inbound message emitted"
                            );
                            emit_window(
                                &window,
                                "chat/inbound",
                                chat_message_event(id, from_viewer, text, ts_unix_ms),
                            );
                        }
                    }
                    Some((ty, body)) if ty == CHAT_MSG_ACK => {
                        if let Ok(ack) = serde_json::from_slice::<ChatAckPayload>(&body) {
                            trace!(
                                target: CHAT_LOG_TARGET,
                                session_id = %session_id,
                                message_id = %ack.message_id,
                                "viewer chat quic ack received"
                            );
                            emit_window(&window, "chat/ack", chat_ack_event(ack.message_id));
                        }
                    }
                    Some((ty, body)) => {
                        trace!(
                            target: CHAT_LOG_TARGET,
                            session_id = %session_id,
                            frame_type = ty,
                            body_len = body.len(),
                            "viewer chat quic ignored frame"
                        );
                    }
                    None => {
                        debug!(
                            target: CHAT_LOG_TARGET,
                            session_id = %session_id,
                            "viewer chat quic stream closed"
                        );
                        break;
                    }
                }
            }
        }
    }

    Ok(())
}

async fn run_chat_relay_session(
    window: Window,
    mut pipe: ChatRelayPipe,
    mut outbound: mpsc::UnboundedReceiver<Vec<u8>>,
    shutdown: CancellationToken,
    session_id: String,
    token: String,
    api_base: String,
) -> Result<(), anyhow::Error> {
    let auth_inner = build_chat_frame(CHAT_MSG_AUTH, token.as_bytes())?;
    write_e2e_frame(
        &mut pipe.writer,
        &pipe.cipher,
        &mut pipe.send_counter,
        &auth_inner,
    )
    .await?;
    trace!(
        target: CHAT_LOG_TARGET,
        session_id = %session_id,
        "viewer chat relay auth frame sent"
    );

    post_chat_viewer_connected(&api_base, &session_id, &token).await?;
    debug!(
        target: CHAT_LOG_TARGET,
        session_id = %session_id,
        "viewer chat relay session marked viewer-connected"
    );

    let mut hb = interval(Duration::from_secs(2));
    hb.set_missed_tick_behavior(MissedTickBehavior::Delay);

    loop {
        tokio::select! {
            biased;
            _ = shutdown.cancelled() => break,
            _ = hb.tick() => {
                write_e2e_frame(
                    &mut pipe.writer,
                    &pipe.cipher,
                    &mut pipe.send_counter,
                    HEARTBEAT_PAYLOAD,
                )
                .await?;
                let url = format!(
                    "{}/api/rmm/chat/session/{}/viewer-heartbeat?token={}",
                    api_base.trim_end_matches('/'),
                    session_id,
                    urlencoding::encode(&token)
                );
                let _ = Client::new().post(url).send().await;
            }
            m = outbound.recv() => {
                let Some(bytes) = m else { break };
                trace!(
                    target: CHAT_LOG_TARGET,
                    session_id = %session_id,
                    frame_len = bytes.len(),
                    "viewer chat relay outbound frame"
                );
                write_e2e_frame(
                    &mut pipe.writer,
                    &pipe.cipher,
                    &mut pipe.send_counter,
                    &bytes,
                )
                .await?;
            }
            p = read_e2e_frame_from(&mut pipe.reader, &pipe.cipher) => {
                let payload = p?;
                if payload == HEARTBEAT_PAYLOAD || payload == b"hello-world" {
                    continue;
                }
                trace!(
                    target: CHAT_LOG_TARGET,
                    session_id = %session_id,
                    payload_len = payload.len(),
                    "viewer chat relay inbound encrypted payload"
                );
                let (ty, body) = talos_protocol::parse_chat_frame(&payload)
                    .map_err(|e| anyhow!("chat relay parse: {e}"))?;
                if ty != CHAT_MSG_TEXT {
                    if ty == CHAT_MSG_ACK {
                        if let Ok(ack) = serde_json::from_slice::<ChatAckPayload>(body) {
                            trace!(
                                target: CHAT_LOG_TARGET,
                                session_id = %session_id,
                                message_id = %ack.message_id,
                                "viewer chat relay ack received"
                            );
                            emit_window(&window, "chat/ack", chat_ack_event(ack.message_id));
                        }
                    }
                    continue;
                }
                if let Ok(ChatWirePayload::Message {
                    id,
                    from_viewer,
                    text,
                    ts_unix_ms,
                }) = serde_json::from_slice(body)
                {
                    let ack = serde_json::to_vec(&ChatAckPayload {
                        message_id: id.clone(),
                    })?;
                    let ack_frame = build_chat_frame(CHAT_MSG_ACK, &ack)?;
                    write_e2e_frame(
                        &mut pipe.writer,
                        &pipe.cipher,
                        &mut pipe.send_counter,
                        &ack_frame,
                    )
                    .await?;
                    debug!(
                        target: CHAT_LOG_TARGET,
                        session_id = %session_id,
                        message_id = %id,
                        from_viewer,
                        text_len = text.len(),
                        "viewer chat relay inbound message emitted"
                    );
                    emit_window(
                        &window,
                        "chat/inbound",
                        chat_message_event(id, from_viewer, text, ts_unix_ms),
                    );
                }
            }
        }
    }

    Ok(())
}

pub async fn chat_connect(
    window: Window,
    chat_state: ChatConnectionState,
    session_id: String,
    token: String,
    api_base: String,
    viewer_transport: String,
    transports: Vec<String>,
    agent_reflex: Option<ReflexAddress>,
    agent_host: Option<String>,
    agent_local_addrs: Option<Vec<LocalAddr>>,
    psk_cert_pem: Option<String>,
    relay_url: Option<String>,
    e2e_key: Option<String>,
    quic_timeout_ms: Option<u64>,
) -> Result<(), String> {
    debug!(
        target: CHAT_LOG_TARGET,
        session_id = %session_id,
        viewer_transport = %viewer_transport,
        transports = ?transports,
        has_agent_reflex = agent_reflex.is_some(),
        has_psk_cert = psk_cert_pem.is_some(),
        has_relay_url = relay_url.is_some(),
        has_e2e_key = e2e_key.is_some(),
        "viewer chat connect requested"
    );
    let _caps = fetch_chat_capabilities(&api_base, &session_id, &token)
        .await
        .map_err(|e| e.to_string())?;

    let connect_started_at = Instant::now();

    {
        let mut guard = chat_state.0.lock().await;
        if let Some(existing) = guard.take() {
            debug!(
                target: CHAT_LOG_TARGET,
                session_id = %session_id,
                "viewer chat replacing existing connection"
            );
            existing.shutdown.cancel();
            existing.join.abort();
        }
    }

    let normalized_transport = viewer_transport.trim().to_ascii_lowercase();
    let supports_quic = transports.iter().any(|t| t == "quic");
    let supports_relay = transports.iter().any(|t| t == "relay");

    enum Prepared {
        Quic(Endpoint, Connection),
        Relay(ChatRelayPipe),
    }

    let prepared: Prepared = if normalized_transport != "tcprelay"
        && supports_quic
        && agent_reflex.is_some()
        && psk_cert_pem.is_some()
    {
        match connect_chat_quic_transport(
            &session_id,
            &token,
            &api_base,
            agent_reflex.as_ref().expect("checked"),
            agent_host.clone(),
            agent_local_addrs.clone(),
            psk_cert_pem.as_ref().expect("checked"),
            quic_timeout_ms,
        )
        .await
        {
            Ok((ep, conn)) => Prepared::Quic(ep, conn),
            Err(err) => {
                if normalized_transport == "quic" {
                    return Err(format!("chat quic connect failed: {err}"));
                }
                warn!(target: CHAT_LOG_TARGET, error = %err, "chat quic failed; falling back to relay");
                if !supports_relay {
                    return Err(format!("chat quic failed and relay not negotiated: {err}"));
                }
                Prepared::Relay(
                    open_chat_relay_session_pipe(
                        &api_base,
                        &session_id,
                        &token,
                        &relay_url,
                        &e2e_key,
                    )
                    .await?,
                )
            }
        }
    } else if supports_relay {
        Prepared::Relay(
            open_chat_relay_session_pipe(&api_base, &session_id, &token, &relay_url, &e2e_key)
                .await?,
        )
    } else {
        return Err("no chat transports available".to_string());
    };

    let shutdown = CancellationToken::new();
    let (outbound_tx, outbound_rx) = mpsc::unbounded_channel();
    let sd = shutdown.clone();

    let (transport_label, join) = match prepared {
        Prepared::Quic(endpoint, connection) => {
            let remote_addr = connection.remote_address().to_string();
            info!(target: CHAT_LOG_TARGET, session_id = %session_id, %remote_addr, "viewer chat quic connected");
            let win = window.clone();
            let sid = session_id.clone();
            let tok = token.clone();
            let api = api_base.clone();
            let token_shutdown = sd.clone();
            (
                "quic",
                tokio::spawn(async move {
                    let _keep_ep = endpoint;
                    if let Err(err) = run_chat_quic_session(
                        win.clone(),
                        connection,
                        outbound_rx,
                        token_shutdown,
                        sid,
                        tok,
                        api,
                    )
                    .await
                    {
                        warn!(
                            target: CHAT_LOG_TARGET,
                            error = %err,
                            "viewer chat quic session ended with error"
                        );
                        emit_window(
                            &win,
                            "chat/status",
                            json!({ "connected": false, "transport": "quic", "error": err.to_string() }),
                        );
                    } else {
                        debug!(
                            target: CHAT_LOG_TARGET,
                            "viewer chat quic session ended"
                        );
                        emit_window(
                            &win,
                            "chat/status",
                            json!({ "connected": false, "transport": "quic" }),
                        );
                    }
                }),
            )
        }
        Prepared::Relay(pipe) => {
            info!(target: CHAT_LOG_TARGET, session_id = %session_id, "viewer chat relay connected");
            let win = window.clone();
            let sid = session_id.clone();
            let tok = token.clone();
            let api = api_base.clone();
            let token_shutdown = sd.clone();
            (
                "relay",
                tokio::spawn(async move {
                    if let Err(err) = run_chat_relay_session(
                        win.clone(),
                        pipe,
                        outbound_rx,
                        token_shutdown,
                        sid,
                        tok,
                        api,
                    )
                    .await
                    {
                        warn!(
                            target: CHAT_LOG_TARGET,
                            error = %err,
                            "viewer chat relay session ended with error"
                        );
                        emit_window(
                            &win,
                            "chat/status",
                            json!({ "connected": false, "transport": "relay", "error": err.to_string() }),
                        );
                    } else {
                        debug!(
                            target: CHAT_LOG_TARGET,
                            "viewer chat relay session ended"
                        );
                        emit_window(
                            &win,
                            "chat/status",
                            json!({ "connected": false, "transport": "relay" }),
                        );
                    }
                }),
            )
        }
    };

    {
        let mut guard = chat_state.0.lock().await;
        *guard = Some(ChatIo {
            shutdown,
            outbound_tx,
            join,
        });
    }

    emit_window(
        &window,
        "chat/status",
        json!({
            "connected": true,
            "transport": transport_label,
            "connectMs": connect_started_at.elapsed().as_millis().min(u128::from(u64::MAX)) as u64,
        }),
    );
    debug!(
        target: CHAT_LOG_TARGET,
        session_id = %session_id,
        transport = %transport_label,
        connect_ms = connect_started_at.elapsed().as_millis().min(u128::from(u64::MAX)) as u64,
        "viewer chat status emitted connected"
    );

    Ok(())
}

pub async fn chat_send_message(
    chat_state: ChatConnectionState,
    text: String,
) -> Result<serde_json::Value, String> {
    let guard = chat_state.0.lock().await;
    let io = guard
        .as_ref()
        .ok_or_else(|| "Chat is not connected".to_string())?;
    let id = uuid::Uuid::new_v4().to_string();
    let ts_unix_ms = Some(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0),
    );
    let payload = ChatWirePayload::Message {
        id: id.clone(),
        from_viewer: true,
        text: text.clone(),
        ts_unix_ms,
    };
    let body = serde_json::to_vec(&payload).map_err(|e| e.to_string())?;
    let frame = build_chat_frame(CHAT_MSG_TEXT, &body).map_err(|e| e.to_string())?;
    io.outbound_tx
        .send(frame)
        .map_err(|_| "Chat pipeline closed".to_string())?;
    debug!(
        target: CHAT_LOG_TARGET,
        message_id = %id,
        text_len = text.len(),
        "viewer chat outbound message queued"
    );
    Ok(chat_message_event(id, true, text, ts_unix_ms))
}

pub async fn chat_disconnect(
    window: Window,
    chat_state: ChatConnectionState,
    api_base: String,
    session_id: String,
    token: String,
) {
    let mut guard = chat_state.0.lock().await;
    if let Some(existing) = guard.take() {
        debug!(
            target: CHAT_LOG_TARGET,
            session_id = %session_id,
            "viewer chat disconnect cancelling active connection"
        );
        existing.shutdown.cancel();
        existing.join.abort();
    }
    drop(guard);

    let url = format!(
        "{}/api/rmm/chat/session/{}/end?token={}",
        api_base.trim_end_matches('/'),
        session_id,
        urlencoding::encode(&token)
    );
    let _ = Client::new().post(url).send().await;
    emit_window(&window, "chat/status", json!({ "connected": false }));
    debug!(
        target: CHAT_LOG_TARGET,
        session_id = %session_id,
        "viewer chat status emitted disconnected"
    );
}
