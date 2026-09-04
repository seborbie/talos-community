use std::{
    collections::HashMap,
    env,
    net::SocketAddr,
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

use anyhow::{anyhow, Context, Result};
use rustls::pki_types::PrivateKeyDer;
use rustls_pemfile::{certs, pkcs8_private_keys, rsa_private_keys};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::Mutex,
    time::sleep,
};
use tokio_rustls::TlsAcceptor;
use tracing::{debug, info, warn};

const RELAY_COPY_BUFFER_BYTES: usize = 64 * 1024;
const RELAY_E2E_NONCE_BYTES: usize = 12;
const RELAY_E2E_MAX_FRAME_BYTES: usize = 128 * 1024 * 1024;
const DEFAULT_RELAY_BIND_ADDR: &str = "127.0.0.1:443";
static NEXT_CONNECTION_ID: AtomicU64 = AtomicU64::new(1);

struct Config {
    bind_addr: SocketAddr,
    tls_cert_path: Option<String>,
    tls_key_path: Option<String>,
    tls_terminated: bool,
    pending_ttl_secs: u64,
    cleanup_interval_secs: u64,
}

trait RelayStream: AsyncRead + AsyncWrite + Unpin + Send {}

impl<T> RelayStream for T where T: AsyncRead + AsyncWrite + Unpin + Send {}

type BoxedRelayStream = Box<dyn RelayStream>;

struct PendingConnection {
    stream: BoxedRelayStream,
    received_at: Instant,
    remote_addr: SocketAddr,
    connection_id: u64,
}

struct RelayState {
    pending: Mutex<HashMap<String, PendingConnection>>,
    pending_ttl: Duration,
}

#[tokio::main]
async fn main() -> Result<()> {
    rustls::crypto::ring::default_provider()
        .install_default()
        .expect("install rustls CryptoProvider once per process");

    load_dotenv();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let config = load_config()?;
    debug!(
        bind_addr = %config.bind_addr,
        tls_cert_path = ?config.tls_cert_path,
        tls_key_path = ?config.tls_key_path,
        tls_terminated = config.tls_terminated,
        pending_ttl_secs = config.pending_ttl_secs,
        cleanup_interval_secs = config.cleanup_interval_secs,
        "relay config loaded"
    );
    let acceptor =
        if config.tls_terminated {
            info!("relay TLS termination disabled; expecting plaintext from upstream proxy");
            None
        } else {
            let cert_path = config.tls_cert_path.as_deref().context(
                "RMM_RELAY_TLS_CERT_PATH must be set unless RMM_RELAY_TLS_TERMINATED=true",
            )?;
            let key_path = config.tls_key_path.as_deref().context(
                "RMM_RELAY_TLS_KEY_PATH must be set unless RMM_RELAY_TLS_TERMINATED=true",
            )?;
            let tls_config = load_tls_config(cert_path, key_path)?;
            debug!("TLS config built successfully (ALPN http/1.1)");
            Some(TlsAcceptor::from(Arc::new(tls_config)))
        };

    let listener = TcpListener::bind(config.bind_addr)
        .await
        .context("bind relay listener")?;
    info!(bind_addr = %config.bind_addr, "relay listening");
    info!("set RUST_LOG=debug for connection-level diagnostics");

    let state = Arc::new(RelayState {
        pending: Mutex::new(HashMap::new()),
        pending_ttl: Duration::from_secs(config.pending_ttl_secs),
    });

    spawn_cleanup_task(
        state.clone(),
        Duration::from_secs(config.cleanup_interval_secs),
    );

    loop {
        let (tcp_stream, remote_addr) = match listener.accept().await {
            Ok(conn) => conn,
            Err(err) => {
                warn!(error = %err, "failed to accept relay connection");
                continue;
            }
        };
        let connection_id = NEXT_CONNECTION_ID.fetch_add(1, Ordering::Relaxed);

        debug!(connection_id, remote = %remote_addr, "TCP connection accepted, spawning handler");

        let acceptor = acceptor.clone();
        let state = state.clone();
        tokio::spawn(async move {
            if let Err(err) =
                handle_connection(tcp_stream, remote_addr, connection_id, acceptor, state).await
            {
                let chain = format_error_chain(&err);
                warn!(
                    error = %err,
                    error_chain = %chain,
                    connection_id,
                    remote = %remote_addr,
                    "relay connection error (client may see 'tls handshake eof')"
                );
                // Log root cause on its own line so it's visible in Docker logs at default level
                if let Some(root) = err.chain().next() {
                    warn!(root_cause = %root, "relay TLS/connection failure detail");
                }
            }
        });
    }
}

fn format_error_chain(err: &anyhow::Error) -> String {
    err.chain()
        .map(|cause| cause.to_string())
        .collect::<Vec<_>>()
        .join(" | caused by: ")
}

fn load_dotenv() {
    let mut candidates = Vec::new();
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    candidates.push(manifest_dir.join("..").join(".env"));
    if let Ok(cwd) = env::current_dir() {
        candidates.push(cwd.join(".env"));
    }
    if let Ok(exe) = env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join("..").join("..").join(".env"));
        }
    }

    for path in candidates {
        if path.exists() {
            match dotenvy::from_path(&path) {
                Ok(_) => {
                    info!(path = %path.display(), "loaded relay env file");
                }
                Err(err) => {
                    warn!(
                        path = %path.display(),
                        error = %err,
                        "failed to load relay env file"
                    );
                }
            }
            load_env_fallback(&path);
            break;
        }
    }
}

fn load_env_fallback(path: &PathBuf) {
    if env::var("RMM_RELAY_TLS_TERMINATED").is_ok()
        && env::var("RMM_RELAY_TLS_CERT_PATH").is_ok()
        && env::var("RMM_RELAY_TLS_KEY_PATH").is_ok()
    {
        return;
    }

    let bytes = match std::fs::read(path) {
        Ok(data) => data,
        Err(err) => {
            warn!(path = %path.display(), error = %err, "failed to read env file");
            return;
        }
    };

    let contents = if bytes.starts_with(&[0xFF, 0xFE]) {
        let u16s = bytes[2..]
            .chunks_exact(2)
            .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
            .collect::<Vec<_>>();
        String::from_utf16(&u16s)
            .map_err(|_| anyhow!("invalid utf16le env file"))
            .ok()
    } else if bytes.starts_with(&[0xFE, 0xFF]) {
        let u16s = bytes[2..]
            .chunks_exact(2)
            .map(|chunk| u16::from_be_bytes([chunk[0], chunk[1]]))
            .collect::<Vec<_>>();
        String::from_utf16(&u16s)
            .map_err(|_| anyhow!("invalid utf16be env file"))
            .ok()
    } else {
        String::from_utf8(bytes).ok()
    };

    let Some(contents) = contents else {
        warn!(path = %path.display(), "failed to parse env file contents");
        return;
    };

    for line in contents.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let Some((key, value)) = trimmed.split_once('=') else {
            continue;
        };
        let key = key.trim();
        if key != "RMM_RELAY_TLS_CERT_PATH"
            && key != "RMM_RELAY_TLS_KEY_PATH"
            && key != "RMM_RELAY_TLS_TERMINATED"
        {
            continue;
        }
        if env::var(key).is_err() {
            let value = value.trim().trim_matches('"');
            env::set_var(key, value);
            info!(key = key, "set relay env var from fallback loader");
        }
    }
}

fn load_config() -> Result<Config> {
    let bind_addr =
        env::var("RMM_RELAY_BIND_ADDR").unwrap_or_else(|_| DEFAULT_RELAY_BIND_ADDR.to_string());
    let bind_addr: SocketAddr = bind_addr.parse().context("parse RMM_RELAY_BIND_ADDR")?;

    let tls_terminated = env::var("RMM_RELAY_TLS_TERMINATED")
        .ok()
        .map(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(false);

    let tls_cert_path = env::var("RMM_RELAY_TLS_CERT_PATH").ok();
    let tls_key_path = env::var("RMM_RELAY_TLS_KEY_PATH").ok();

    if !tls_terminated {
        tls_cert_path
            .as_ref()
            .context("RMM_RELAY_TLS_CERT_PATH must be set unless RMM_RELAY_TLS_TERMINATED=true")?;
        tls_key_path
            .as_ref()
            .context("RMM_RELAY_TLS_KEY_PATH must be set unless RMM_RELAY_TLS_TERMINATED=true")?;
    }

    let pending_ttl_secs = env::var("RMM_RELAY_PENDING_TTL_SECS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(600);

    let cleanup_interval_secs = env::var("RMM_RELAY_CLEANUP_INTERVAL_SECS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(30);

    Ok(Config {
        bind_addr,
        tls_cert_path,
        tls_key_path,
        tls_terminated,
        pending_ttl_secs,
        cleanup_interval_secs,
    })
}

fn load_tls_config(cert_path: &str, key_path: &str) -> Result<rustls::ServerConfig> {
    debug!(cert_path = %cert_path, key_path = %key_path, "reading TLS cert and key files");
    let cert_pem = std::fs::read(cert_path).context("read relay tls cert")?;
    let key_pem = std::fs::read(key_path).context("read relay tls key")?;
    debug!(
        cert_bytes = cert_pem.len(),
        key_bytes = key_pem.len(),
        "TLS files read"
    );

    let mut cert_reader = std::io::Cursor::new(cert_pem);
    let certs = certs(&mut cert_reader)
        .collect::<std::io::Result<Vec<_>>>()
        .context("parse relay tls cert")?;
    let cert_count = certs.len();
    let certs = certs.into_iter().collect();
    debug!(cert_count, "parsed certificate chain");

    let mut key_reader = std::io::Cursor::new(&key_pem);
    let key = pkcs8_private_keys(&mut key_reader)
        .collect::<std::io::Result<Vec<_>>>()
        .context("parse relay tls key (pkcs8)")?
        .into_iter()
        .next()
        .map(PrivateKeyDer::from)
        .or_else(|| {
            let rsa_keys: Vec<_> =
                match rsa_private_keys(&mut std::io::Cursor::new(&key_pem)).collect() {
                    Ok(k) => k,
                    Err(_) => return None,
                };
            rsa_keys.into_iter().next().map(PrivateKeyDer::from)
        })
        .context("no relay tls key found (need PKCS#8 or PKCS#1 RSA PEM)")?;
    debug!("parsed private key (PKCS#8 or PKCS#1 RSA)");

    let mut config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .context("build relay tls config")?;
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    debug!(alpn = ?config.alpn_protocols, "ServerConfig built");
    Ok(config)
}

async fn handle_connection(
    tcp_stream: TcpStream,
    remote_addr: SocketAddr,
    connection_id: u64,
    acceptor: Option<TlsAcceptor>,
    state: Arc<RelayState>,
) -> Result<()> {
    debug!(connection_id, remote = %remote_addr, "handle_connection: setting TCP_NODELAY");
    tcp_stream.set_nodelay(true).context("set TCP_NODELAY")?;

    let mut stream: BoxedRelayStream = if let Some(acceptor) = acceptor {
        debug!(connection_id, remote = %remote_addr, "handle_connection: starting TLS accept");
        let stream = acceptor.accept(tcp_stream).await.context("tls accept (if client sees 'tls handshake eof', check this relay log for the underlying error)")?;
        debug!(connection_id, remote = %remote_addr, "handle_connection: TLS handshake complete");
        Box::new(stream)
    } else {
        debug!(connection_id, remote = %remote_addr, "handle_connection: using plaintext stream from TLS-terminating proxy");
        Box::new(tcp_stream)
    };

    debug!(connection_id, remote = %remote_addr, "handle_connection: reading request line");
    let request_line = read_request_line(&mut stream).await?;
    debug!(connection_id, remote = %remote_addr, request_line = %request_line.trim(), "handle_connection: request line received");
    let session_id = parse_session_id(&request_line)?;
    debug!(connection_id, remote = %remote_addr, session_id = %session_id, "handle_connection: session_id parsed");
    debug!(connection_id, remote = %remote_addr, session_id = %session_id, "handle_connection: reading headers");
    read_headers(&mut stream).await?;
    debug!(connection_id, remote = %remote_addr, session_id = %session_id, "handle_connection: headers done, sending 200 OK");
    stream
        .write_all(b"HTTP/1.1 200 OK\r\n\r\n")
        .await
        .context("write relay response")?;
    stream.flush().await.context("flush relay response")?;
    debug!(connection_id, remote = %remote_addr, session_id = %session_id, "relay HTTP 200 response sent");

    let mut pending = state.pending.lock().await;
    if let Some(existing) = pending.remove(&session_id) {
        let pending_age_ms = existing.received_at.elapsed().as_millis() as u64;
        let pending_remote = existing.remote_addr;
        let pending_connection_id = existing.connection_id;
        drop(pending);
        info!(
            session_id = %session_id,
            pending_remote = %pending_remote,
            current_remote = %remote_addr,
            pending_connection_id,
            current_connection_id = connection_id,
            pending_age_ms,
            "relay pairing complete"
        );
        debug!(
            session_id = %session_id,
            pending_remote = %pending_remote,
            current_remote = %remote_addr,
            pending_connection_id,
            current_connection_id = connection_id,
            "relay: starting logged bidirectional copy"
        );
        let mut stream_a = existing.stream;
        let mut stream_b = stream;
        match relay_bidirectional_copy(
            &session_id,
            ConnectionMeta {
                role: "pending",
                remote_addr: pending_remote,
                connection_id: pending_connection_id,
            },
            &mut stream_a,
            ConnectionMeta {
                role: "current",
                remote_addr,
                connection_id,
            },
            &mut stream_b,
        )
        .await
        {
            Ok(stats) => {
                info!(
                    session_id = %session_id,
                    pending_remote = %pending_remote,
                    current_remote = %remote_addr,
                    pending_connection_id,
                    current_connection_id = connection_id,
                    pending_to_current_bytes = stats.a_to_b.bytes,
                    current_to_pending_bytes = stats.b_to_a.bytes,
                    pending_to_current_frames_started = stats.a_to_b.frames_started,
                    pending_to_current_frames_completed = stats.a_to_b.frames_completed,
                    current_to_pending_frames_started = stats.b_to_a.frames_started,
                    current_to_pending_frames_completed = stats.b_to_a.frames_completed,
                    elapsed_ms = stats.elapsed.as_millis() as u64,
                    "relay bidirectional copy ended"
                );
            }
            Err(err) => {
                warn!(
                    session_id = %session_id,
                    pending_remote = %pending_remote,
                    current_remote = %remote_addr,
                    pending_connection_id,
                    current_connection_id = connection_id,
                    error = %err,
                    error_chain = %format_error_chain(&err),
                    "relay bidirectional copy failed"
                );
                return Err(err);
            }
        }
    } else {
        pending.insert(
            session_id.clone(),
            PendingConnection {
                stream,
                received_at: Instant::now(),
                remote_addr,
                connection_id,
            },
        );
        let pending_count = pending.len();
        info!(
            session_id = %session_id,
            remote = %remote_addr,
            connection_id,
            pending_count,
            "relay awaiting peer"
        );
        debug!(connection_id, remote = %remote_addr, session_id = %session_id, pending_count, "relay: inserted into pending map");
    }

    Ok(())
}

#[derive(Clone, Copy)]
struct ConnectionMeta {
    role: &'static str,
    remote_addr: SocketAddr,
    connection_id: u64,
}

struct RelayCopyStats {
    a_to_b: DirectionCopyStats,
    b_to_a: DirectionCopyStats,
    elapsed: Duration,
}

#[derive(Default)]
struct DirectionCopyStats {
    bytes: u64,
    chunks: u64,
    frames_started: u64,
    frames_completed: u64,
}

struct DirectionState {
    label: &'static str,
    from: ConnectionMeta,
    to: ConnectionMeta,
    stats: DirectionCopyStats,
    eof: bool,
    probe: RelayFrameProbe,
}

#[derive(Clone, Copy)]
struct DirectionLogContext {
    label: &'static str,
    from: ConnectionMeta,
    to: ConnectionMeta,
}

impl DirectionState {
    fn new(label: &'static str, from: ConnectionMeta, to: ConnectionMeta) -> Self {
        Self {
            label,
            from,
            to,
            stats: DirectionCopyStats::default(),
            eof: false,
            probe: RelayFrameProbe::new(),
        }
    }

    fn observe_read(&mut self, session_id: &str, bytes: &[u8]) {
        self.stats.bytes = self.stats.bytes.saturating_add(bytes.len() as u64);
        self.stats.chunks = self.stats.chunks.saturating_add(1);
        debug!(
            session_id = %session_id,
            direction = self.label,
            from_role = self.from.role,
            from_remote = %self.from.remote_addr,
            from_connection_id = self.from.connection_id,
            to_role = self.to.role,
            to_remote = %self.to.remote_addr,
            to_connection_id = self.to.connection_id,
            chunk_bytes = bytes.len(),
            direction_bytes = self.stats.bytes,
            direction_chunks = self.stats.chunks,
            "relay forwarding encrypted bytes"
        );
        self.probe.observe(session_id, self.log_context(), bytes);
        self.stats.frames_started = self.probe.frames_started();
        self.stats.frames_completed = self.probe.frames_completed();
    }

    fn observe_eof(&mut self, session_id: &str) {
        self.eof = true;
        self.probe
            .log_terminal_state(session_id, self.log_context(), "eof");
        self.stats.frames_started = self.probe.frames_started();
        self.stats.frames_completed = self.probe.frames_completed();
        info!(
            session_id = %session_id,
            direction = self.label,
            from_role = self.from.role,
            from_remote = %self.from.remote_addr,
            from_connection_id = self.from.connection_id,
            to_role = self.to.role,
            to_remote = %self.to.remote_addr,
            to_connection_id = self.to.connection_id,
            direction_bytes = self.stats.bytes,
            direction_chunks = self.stats.chunks,
            frames_started = self.stats.frames_started,
            frames_completed = self.stats.frames_completed,
            "relay direction reached EOF"
        );
    }

    fn log_context(&self) -> DirectionLogContext {
        DirectionLogContext {
            label: self.label,
            from: self.from,
            to: self.to,
        }
    }
}

struct RelayFrameProbe {
    next_frame_index: u64,
    current_frame_index: u64,
    completed_frames: u64,
    header: [u8; 4],
    header_len: usize,
    expected_frame_len: Option<usize>,
    current_frame_seen: usize,
    current_frame_started_at: Option<Instant>,
}

impl RelayFrameProbe {
    fn new() -> Self {
        Self {
            next_frame_index: 0,
            current_frame_index: 0,
            completed_frames: 0,
            header: [0; 4],
            header_len: 0,
            expected_frame_len: None,
            current_frame_seen: 0,
            current_frame_started_at: None,
        }
    }

    fn frames_started(&self) -> u64 {
        self.next_frame_index
    }

    fn frames_completed(&self) -> u64 {
        self.completed_frames
    }

    fn observe(&mut self, session_id: &str, direction: DirectionLogContext, mut bytes: &[u8]) {
        while !bytes.is_empty() {
            if self.expected_frame_len.is_none() {
                let needed = 4usize.saturating_sub(self.header_len);
                let take = needed.min(bytes.len());
                self.header[self.header_len..self.header_len + take]
                    .copy_from_slice(&bytes[..take]);
                self.header_len += take;
                bytes = &bytes[take..];

                if self.header_len < 4 {
                    continue;
                }

                let frame_len = u32::from_be_bytes(self.header) as usize;
                self.header_len = 0;
                self.next_frame_index = self.next_frame_index.saturating_add(1);
                self.current_frame_index = self.next_frame_index;
                self.expected_frame_len = Some(frame_len);
                self.current_frame_seen = 0;
                self.current_frame_started_at = Some(Instant::now());

                if !(RELAY_E2E_NONCE_BYTES..=RELAY_E2E_MAX_FRAME_BYTES).contains(&frame_len) {
                    warn!(
                        session_id = %session_id,
                        direction = direction.label,
                        from_role = direction.from.role,
                        from_remote = %direction.from.remote_addr,
                        from_connection_id = direction.from.connection_id,
                        to_role = direction.to.role,
                        to_remote = %direction.to.remote_addr,
                        to_connection_id = direction.to.connection_id,
                        frame_index = self.current_frame_index,
                        frame_payload_bytes = frame_len,
                        max_frame_bytes = RELAY_E2E_MAX_FRAME_BYTES,
                        "relay observed suspicious encrypted frame length"
                    );
                } else {
                    debug!(
                        session_id = %session_id,
                        direction = direction.label,
                        from_role = direction.from.role,
                        from_remote = %direction.from.remote_addr,
                        from_connection_id = direction.from.connection_id,
                        to_role = direction.to.role,
                        to_remote = %direction.to.remote_addr,
                        to_connection_id = direction.to.connection_id,
                        frame_index = self.current_frame_index,
                        frame_payload_bytes = frame_len,
                        encrypted_wire_bytes = frame_len.saturating_add(4),
                        "relay observed encrypted frame header"
                    );
                }

                if frame_len == 0 {
                    self.complete_current_frame(session_id, direction);
                }
                continue;
            }

            let expected = self.expected_frame_len.unwrap_or_default();
            let remaining = expected.saturating_sub(self.current_frame_seen);
            let take = remaining.min(bytes.len());
            self.current_frame_seen = self.current_frame_seen.saturating_add(take);
            bytes = &bytes[take..];

            if self.current_frame_seen >= expected {
                self.complete_current_frame(session_id, direction);
            }
        }
    }

    fn complete_current_frame(&mut self, session_id: &str, direction: DirectionLogContext) {
        let expected = self.expected_frame_len.take().unwrap_or_default();
        let elapsed_ms = self
            .current_frame_started_at
            .take()
            .map(|started| started.elapsed().as_millis() as u64)
            .unwrap_or_default();
        self.completed_frames = self.completed_frames.saturating_add(1);
        debug!(
            session_id = %session_id,
            direction = direction.label,
            from_role = direction.from.role,
            from_remote = %direction.from.remote_addr,
            from_connection_id = direction.from.connection_id,
            to_role = direction.to.role,
            to_remote = %direction.to.remote_addr,
            to_connection_id = direction.to.connection_id,
            frame_index = self.current_frame_index,
            frame_payload_bytes = expected,
            encrypted_wire_bytes = expected.saturating_add(4),
            frame_elapsed_ms = elapsed_ms,
            completed_frames = self.completed_frames,
            "relay observed encrypted frame complete"
        );
        self.current_frame_seen = 0;
        self.current_frame_index = 0;
    }

    fn log_terminal_state(
        &self,
        session_id: &str,
        direction: DirectionLogContext,
        terminal_reason: &'static str,
    ) {
        if self.header_len > 0 {
            warn!(
                session_id = %session_id,
                direction = direction.label,
                from_role = direction.from.role,
                from_remote = %direction.from.remote_addr,
                from_connection_id = direction.from.connection_id,
                to_role = direction.to.role,
                to_remote = %direction.to.remote_addr,
                to_connection_id = direction.to.connection_id,
                terminal_reason,
                partial_header_bytes = self.header_len,
                frames_started = self.frames_started(),
                frames_completed = self.frames_completed(),
                "relay stream ended mid encrypted frame header"
            );
        }

        if let Some(expected) = self.expected_frame_len {
            warn!(
                session_id = %session_id,
                direction = direction.label,
                from_role = direction.from.role,
                from_remote = %direction.from.remote_addr,
                from_connection_id = direction.from.connection_id,
                to_role = direction.to.role,
                to_remote = %direction.to.remote_addr,
                to_connection_id = direction.to.connection_id,
                terminal_reason,
                frame_index = self.current_frame_index,
                frame_payload_bytes = expected,
                frame_payload_seen = self.current_frame_seen,
                frame_payload_remaining = expected.saturating_sub(self.current_frame_seen),
                frames_started = self.frames_started(),
                frames_completed = self.frames_completed(),
                "relay stream ended mid encrypted frame payload"
            );
        }
    }
}

async fn relay_bidirectional_copy(
    session_id: &str,
    a_meta: ConnectionMeta,
    stream_a: &mut BoxedRelayStream,
    b_meta: ConnectionMeta,
    stream_b: &mut BoxedRelayStream,
) -> Result<RelayCopyStats> {
    let started = Instant::now();
    let mut a_to_b = DirectionState::new("pending_to_current", a_meta, b_meta);
    let mut b_to_a = DirectionState::new("current_to_pending", b_meta, a_meta);
    let mut a_buf = vec![0u8; RELAY_COPY_BUFFER_BYTES];
    let mut b_buf = vec![0u8; RELAY_COPY_BUFFER_BYTES];

    loop {
        if a_to_b.eof && b_to_a.eof {
            break;
        }

        tokio::select! {
            result = stream_a.read(&mut a_buf), if !a_to_b.eof => {
                let read = result.with_context(|| format!("relay read {}", a_to_b.label))?;
                if read == 0 {
                    a_to_b.observe_eof(session_id);
                    if let Err(err) = stream_b.shutdown().await {
                        warn!(
                            session_id = %session_id,
                            direction = a_to_b.label,
                            target_role = b_meta.role,
                            target_remote = %b_meta.remote_addr,
                            target_connection_id = b_meta.connection_id,
                            error = %err,
                            "relay shutdown after EOF failed"
                        );
                    }
                    continue;
                }
                a_to_b.observe_read(session_id, &a_buf[..read]);
                stream_b
                    .write_all(&a_buf[..read])
                    .await
                    .with_context(|| format!("relay write {}", a_to_b.label))?;
            }
            result = stream_b.read(&mut b_buf), if !b_to_a.eof => {
                let read = result.with_context(|| format!("relay read {}", b_to_a.label))?;
                if read == 0 {
                    b_to_a.observe_eof(session_id);
                    if let Err(err) = stream_a.shutdown().await {
                        warn!(
                            session_id = %session_id,
                            direction = b_to_a.label,
                            target_role = a_meta.role,
                            target_remote = %a_meta.remote_addr,
                            target_connection_id = a_meta.connection_id,
                            error = %err,
                            "relay shutdown after EOF failed"
                        );
                    }
                    continue;
                }
                b_to_a.observe_read(session_id, &b_buf[..read]);
                stream_a
                    .write_all(&b_buf[..read])
                    .await
                    .with_context(|| format!("relay write {}", b_to_a.label))?;
            }
        }
    }

    Ok(RelayCopyStats {
        a_to_b: a_to_b.stats,
        b_to_a: b_to_a.stats,
        elapsed: started.elapsed(),
    })
}

async fn read_request_line<S>(stream: &mut S) -> Result<String>
where
    S: AsyncRead + Unpin + ?Sized,
{
    debug!("read_request_line: start");
    let mut line = Vec::new();
    loop {
        let mut byte = [0u8; 1];
        let read = stream.read(&mut byte).await.context("read request line")?;
        if read == 0 {
            return Err(anyhow!("connection closed while reading request line"));
        }
        line.push(byte[0]);
        if byte[0] == b'\n' {
            break;
        }
        if line.len() > 4096 {
            return Err(anyhow!("request line too long"));
        }
    }

    Ok(String::from_utf8_lossy(&line).trim().to_string())
}

async fn read_headers<S>(stream: &mut S) -> Result<()>
where
    S: AsyncRead + Unpin + ?Sized,
{
    debug!("read_headers: start");
    let mut window = [0u8; 4];
    let mut filled = 0usize;
    loop {
        let mut byte = [0u8; 1];
        let read = stream.read(&mut byte).await.context("read headers")?;
        if read == 0 {
            return Err(anyhow!("connection closed while reading headers"));
        }

        if filled < 4 {
            window[filled] = byte[0];
            filled += 1;
        } else {
            window.rotate_left(1);
            window[3] = byte[0];
        }

        if filled == 4 && window == [b'\r', b'\n', b'\r', b'\n'] {
            break;
        }
    }
    debug!("read_headers: CRLFCRLF seen");
    Ok(())
}

fn parse_session_id(request_line: &str) -> Result<String> {
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or_default();
    let path = parts.next().unwrap_or_default();
    if method != "GET" {
        return Err(anyhow!("unsupported method"));
    }

    let prefix = "/relay/";
    if !path.starts_with(prefix) {
        return Err(anyhow!("invalid relay path"));
    }

    let session_id = &path[prefix.len()..];
    if session_id.is_empty() {
        return Err(anyhow!("missing session id"));
    }

    Ok(session_id.to_string())
}

fn spawn_cleanup_task(state: Arc<RelayState>, interval: Duration) {
    tokio::spawn(async move {
        loop {
            sleep(interval).await;
            let expired = {
                let mut pending = state.pending.lock().await;
                let now = Instant::now();
                let mut expired = Vec::new();
                let count_before = pending.len();
                pending.retain(|session_id, entry| {
                    let keep = now.duration_since(entry.received_at) <= state.pending_ttl;
                    if !keep {
                        expired.push(session_id.clone());
                    }
                    keep
                });
                debug!(
                    pending_before = count_before,
                    pending_after = pending.len(),
                    expired_count = expired.len(),
                    "cleanup pass"
                );
                expired
            };
            for session_id in expired {
                info!(session_id = %session_id, "relay pending connection expired");
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_native_default_bind_is_loopback_only() {
        let address = DEFAULT_RELAY_BIND_ADDR
            .parse::<SocketAddr>()
            .expect("default relay address must parse");

        assert!(address.ip().is_loopback());
    }
}
