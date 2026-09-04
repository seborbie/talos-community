//! Shared relay transport helpers (client-side).
//!
//! This module is feature-gated because it pulls in TLS + async I/O dependencies.
//! Enable with the `relay-transport` crate feature.

use std::{fs, sync::Arc};

use anyhow::{anyhow, Context, Result};
use chacha20poly1305::{aead::Aead, ChaCha20Poly1305, Key, KeyInit, Nonce};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::client::WebPkiServerVerifier;
use rustls::pki_types::{CertificateDer, ServerName};
use rustls::{ClientConfig, RootCertStore};
use rustls_native_certs::load_native_certs;
use rustls_pemfile::certs;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use url::Url;
use webpki_roots::TLS_SERVER_ROOTS;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelayTarget {
    pub host: String,
    pub port: u16,
}

pub fn parse_relay_target(relay_url: &str) -> Result<RelayTarget> {
    let url = if relay_url.contains("://") {
        Url::parse(relay_url).context("parse relay url")?
    } else {
        Url::parse(&format!("https://{relay_url}")).context("parse relay url")?
    };

    let host = url
        .host_str()
        .ok_or_else(|| anyhow!("relay url missing host"))?
        .to_string();
    let port = url.port_or_known_default().unwrap_or(443);
    Ok(RelayTarget { host, port })
}

/// When using a custom relay CA, the relay cert is often issued for one name (e.g. "relay.local")
/// while the client connects via a different public TCP endpoint. This verifier trusts
/// the chain but validates the certificate against the overridden name.
#[derive(Debug)]
struct RelayHostnameVerifier {
    inner: Arc<WebPkiServerVerifier>,
    verify_as: String,
}

impl RelayHostnameVerifier {
    fn new(roots: RootCertStore, verify_as: &str) -> Result<Self> {
        let inner = WebPkiServerVerifier::builder(Arc::new(roots))
            .build()
            .map_err(|e| anyhow!("relay verifier build: {e}"))?;
        Ok(Self {
            inner,
            verify_as: verify_as.to_string(),
        })
    }
}

impl ServerCertVerifier for RelayHostnameVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        ocsp_response: &[u8],
        now: rustls::pki_types::UnixTime,
    ) -> std::result::Result<ServerCertVerified, rustls::Error> {
        let verify_as = ServerName::try_from(self.verify_as.clone()).map_err(|_| {
            rustls::Error::General(format!("invalid relay verify_as name: {}", self.verify_as))
        })?;
        self.inner
            .verify_server_cert(end_entity, intermediates, &verify_as, ocsp_response, now)
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        self.inner.verify_tls12_signature(message, cert, dss)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        self.inner.verify_tls13_signature(message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.inner.supported_verify_schemes()
    }
}

/// Build a rustls client config for connecting to the relay.
///
/// - Always trusts the standard WebPKI root store.
/// - If `relay_ca_path` is provided, also trusts that CA.
/// - If `verify_hostname_override` is provided, uses a custom verifier that validates the
///   certificate against that name (not the actual TCP host).
pub fn build_relay_client_tls_config(
    relay_ca_path: Option<&str>,
    verify_hostname_override: Option<&str>,
) -> Result<ClientConfig> {
    let mut roots = RootCertStore::empty();

    let native_roots = load_native_certs();
    for cert in native_roots.certs {
        let _ = roots.add(cert);
    }

    roots.extend(TLS_SERVER_ROOTS.iter().cloned());

    if let Some(path) = relay_ca_path {
        let pem = fs::read(path).context("read relay CA PEM")?;
        let mut cursor = std::io::Cursor::new(pem);
        for cert in certs(&mut cursor) {
            let cert = cert.map_err(|e| anyhow!("relay CA PEM: {e}"))?;
            roots.add(cert).map_err(|e| anyhow!("add relay CA: {e}"))?;
        }
    }

    let mut config = match verify_hostname_override {
        Some(verify_as) => {
            // rustls currently requires the `dangerous()` builder to install a custom verifier.
            let verifier = RelayHostnameVerifier::new(roots, verify_as)?;
            ClientConfig::builder()
                .dangerous()
                .with_custom_certificate_verifier(Arc::new(verifier))
                .with_no_client_auth()
        }
        None => ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth(),
    };
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(config)
}

/// Read an HTTP response head (until CRLFCRLF) from an async stream.
///
/// This does NOT parse the status code; it only consumes the header bytes so the caller can
/// proceed with framed binary I/O over the same TCP/TLS stream.
pub async fn read_http_response<R: AsyncRead + Unpin>(stream: &mut R) -> Result<()> {
    const MAX_HEADER_BYTES: usize = 64 * 1024; // hard cap to prevent unbounded reads
    let mut window = [0u8; 4];
    let mut filled = 0usize;
    let mut read_total: usize = 0;
    loop {
        let mut byte = [0u8; 1];
        let read = stream
            .read(&mut byte)
            .await
            .context("read relay response")?;
        if read == 0 {
            return Err(anyhow!("relay connection closed while reading response"));
        }
        read_total = read_total.saturating_add(1);
        if read_total > MAX_HEADER_BYTES {
            return Err(anyhow!("relay http response headers too large"));
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
    Ok(())
}

pub fn build_e2e_cipher(key_bytes: &[u8]) -> Result<ChaCha20Poly1305> {
    if key_bytes.len() != 32 {
        return Err(anyhow!("invalid e2e key length"));
    }
    Ok(ChaCha20Poly1305::new(Key::from_slice(key_bytes)))
}

fn nonce_from_counter(counter: u64) -> [u8; 12] {
    let mut nonce = [0u8; 12];
    nonce[4..].copy_from_slice(&counter.to_be_bytes());
    nonce
}

pub async fn write_e2e_frame<W: AsyncWrite + Unpin>(
    stream: &mut W,
    cipher: &ChaCha20Poly1305,
    counter: &mut u64,
    plaintext: &[u8],
) -> Result<()> {
    let nonce_bytes = nonce_from_counter(*counter);
    *counter = counter.saturating_add(1);
    let nonce = Nonce::from_slice(&nonce_bytes);
    let ciphertext = cipher
        .encrypt(nonce, plaintext)
        .map_err(|err| anyhow!("encrypt relay frame: {err}"))?;
    let frame_len = (nonce_bytes.len() + ciphertext.len()) as u32;
    stream
        .write_all(&frame_len.to_be_bytes())
        .await
        .context("write relay frame length")?;
    stream
        .write_all(&nonce_bytes)
        .await
        .context("write relay nonce")?;
    stream
        .write_all(&ciphertext)
        .await
        .context("write relay ciphertext")?;
    Ok(())
}

pub async fn write_e2e_frame_flush<W: AsyncWrite + Unpin>(
    stream: &mut W,
    cipher: &ChaCha20Poly1305,
    counter: &mut u64,
    plaintext: &[u8],
) -> Result<()> {
    write_e2e_frame(stream, cipher, counter, plaintext).await?;
    stream.flush().await.context("flush relay frame")?;
    Ok(())
}

pub async fn read_e2e_frame_from<R: AsyncRead + Unpin>(
    reader: &mut R,
    cipher: &ChaCha20Poly1305,
) -> Result<Vec<u8>> {
    const MAX_FRAME_BYTES: usize = 128 * 1024 * 1024; // cap to prevent OOM on corrupt streams
    let mut len_bytes = [0u8; 4];
    reader
        .read_exact(&mut len_bytes)
        .await
        .context("read relay frame length")?;
    let frame_len = u32::from_be_bytes(len_bytes) as usize;
    if frame_len < 12 {
        return Err(anyhow!("invalid relay frame length"));
    }
    if frame_len > MAX_FRAME_BYTES {
        return Err(anyhow!("relay frame too large"));
    }
    let mut frame = vec![0u8; frame_len];
    reader
        .read_exact(&mut frame)
        .await
        .context("read relay frame")?;
    let (nonce_bytes, ciphertext) = frame.split_at(12);
    let nonce = Nonce::from_slice(nonce_bytes);
    cipher
        .decrypt(nonce, ciphertext)
        .map_err(|err| anyhow!("decrypt relay frame: {err}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_relay_target_accepts_missing_scheme() {
        let t = parse_relay_target("example.com:443").expect("parse");
        assert_eq!(t.host, "example.com");
        assert_eq!(t.port, 443);
    }

    #[test]
    fn parse_relay_target_accepts_https_url() {
        let t = parse_relay_target("https://example.com:8443").expect("parse");
        assert_eq!(t.host, "example.com");
        assert_eq!(t.port, 8443);
    }
}
