//! Windows desktop relay session transport.
use super::*;

pub(super) async fn start_relay_client_once(
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
