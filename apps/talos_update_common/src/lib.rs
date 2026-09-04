use std::{
    fs::File,
    io::{BufReader, Read, Write},
    path::Path,
};

use anyhow::{anyhow, Context, Result};
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use base64::Engine as _;
use reqwest::header::{ETAG, IF_NONE_MATCH};
use ring::signature;
use semver::Version;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const MANIFEST_SIGNATURE_HEADER: &str = "x-talos-manifest-signature";
pub const MANIFEST_KEY_ID_HEADER: &str = "x-talos-manifest-key-id";
const MAX_MANIFEST_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdatePackageArtifact {
    pub file_name: String,
    pub size_bytes: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateManifest {
    pub product: String,
    pub platform: String,
    pub arch: String,
    pub channel: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ring: Option<String>,
    pub version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minimum_supported_version: Option<String>,
    pub severity: String,
    pub published_at_utc: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rollout_percentage: Option<u8>,
    pub package: UpdatePackageArtifact,
    #[serde(default)]
    pub contents: Vec<String>,
    pub requires_restart: bool,
    pub install_mode: String,
}

#[derive(Debug, Clone)]
pub struct SignedManifest {
    pub manifest: UpdateManifest,
    pub manifest_bytes: Vec<u8>,
    pub signature_b64: String,
    pub key_id: Option<String>,
    pub etag: Option<String>,
}

#[derive(Debug, Clone)]
pub enum ManifestFetchResult {
    NotModified,
    NoUpdate,
    Signed(SignedManifest),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateManifestExpectation {
    pub product: String,
    pub platform: String,
    pub arch: String,
    pub channel: String,
    pub ring: Option<String>,
    pub install_mode: String,
    pub package_file_name: String,
}

impl UpdateManifestExpectation {
    pub fn for_artifact(
        product: &str,
        arch: &str,
        channel: &str,
        ring: Option<&str>,
    ) -> Result<Self> {
        let platform = match arch {
            "x86" | "x64" | "x64-v1" | "x64-v2" | "x64-v3" | "x64-v4" => "windows",
            value if value.starts_with("linux-") => "linux",
            value if value.starts_with("macos-") => "macos",
            _ => return Err(anyhow!("unsupported update architecture '{arch}'")),
        };
        let product_label = match product {
            "agent" => "Agent",
            "viewer" => "Viewer",
            "worker" => "Worker",
            "supervisor" => "Supervisor",
            _ => return Err(anyhow!("unsupported update product '{product}'")),
        };
        let install_mode = match (product, platform) {
            ("viewer", "windows") => "restart",
            ("viewer", "macos") => "pkg",
            ("worker" | "supervisor", "macos") => "zip",
            ("agent" | "worker" | "supervisor", "windows" | "linux") => "silent",
            _ => {
                return Err(anyhow!(
                    "unsupported update product/platform combination '{product}/{platform}'"
                ))
            }
        };
        let package_file_name = if product == "viewer" && platform == "macos" {
            "Talos.Viewer.macos.pkg".to_string()
        } else {
            format!("Talos.{product_label}.{arch}.Update.zip")
        };
        Ok(Self {
            product: product.to_string(),
            platform: platform.to_string(),
            arch: arch.to_string(),
            channel: channel.to_string(),
            ring: ring.map(ToString::to_string),
            install_mode: install_mode.to_string(),
            package_file_name,
        })
    }
}

pub fn validate_manifest_context(
    manifest: &UpdateManifest,
    expected: &UpdateManifestExpectation,
) -> Result<()> {
    for (label, actual, expected) in [
        (
            "product",
            manifest.product.as_str(),
            expected.product.as_str(),
        ),
        (
            "platform",
            manifest.platform.as_str(),
            expected.platform.as_str(),
        ),
        (
            "architecture",
            manifest.arch.as_str(),
            expected.arch.as_str(),
        ),
        (
            "channel",
            manifest.channel.as_str(),
            expected.channel.as_str(),
        ),
        (
            "install mode",
            manifest.install_mode.as_str(),
            expected.install_mode.as_str(),
        ),
        (
            "package file name",
            manifest.package.file_name.as_str(),
            expected.package_file_name.as_str(),
        ),
    ] {
        if actual != expected {
            return Err(anyhow!(
                "signed update manifest {label} does not match the requested artifact context"
            ));
        }
    }
    if manifest.ring.as_deref() != expected.ring.as_deref() {
        return Err(anyhow!(
            "signed update manifest ring does not match the requested artifact context"
        ));
    }
    validate_package_size(manifest.package.size_bytes)?;
    validate_sha256(&manifest.package.sha256)?;
    Ok(())
}

pub fn normalize_semver(value: &str) -> Result<Version> {
    let trimmed = value.trim();
    let normalized = trimmed.strip_prefix('v').unwrap_or(trimmed);
    Version::parse(normalized).with_context(|| format!("parse version '{value}'"))
}

pub fn is_update_newer(current_version: &str, next_version: &str) -> Result<bool> {
    let current = normalize_semver(current_version)?;
    let next = normalize_semver(next_version)?;
    Ok(next > current)
}

pub fn normalize_update_base_url(value: &str) -> Option<String> {
    let trimmed = value.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return None;
    }
    let parsed = reqwest::Url::parse(trimmed).ok()?;
    if !matches!(parsed.scheme(), "http" | "https")
        || parsed.host_str().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        return None;
    }
    if trimmed.ends_with("/rmm/updates") {
        Some(trimmed.to_string())
    } else {
        Some(format!("{trimmed}/rmm/updates"))
    }
}

pub fn verify_manifest_signature(
    public_key_der: &[u8],
    manifest_bytes: &[u8],
    signature_b64: &str,
) -> Result<()> {
    if public_key_der.is_empty() {
        return Err(anyhow!("manifest signing public key is not embedded"));
    }
    let signature = BASE64_STANDARD
        .decode(signature_b64)
        .context("decode base64 manifest signature")?;
    let verifier =
        signature::UnparsedPublicKey::new(&signature::RSA_PKCS1_2048_8192_SHA256, public_key_der);
    verifier
        .verify(manifest_bytes, &signature)
        .map_err(|_| anyhow!("manifest signature verification failed"))?;
    Ok(())
}

pub fn sha256_hex_bytes(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex_lower(&hasher.finalize())
}

pub fn sha256_hex_file(path: &Path) -> Result<String> {
    let file = File::open(path).with_context(|| format!("open {}", path.display()))?;
    let mut reader = BufReader::new(file);
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = reader
            .read(&mut buffer)
            .with_context(|| format!("read {}", path.display()))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex_lower(&hasher.finalize()))
}

pub fn verify_package_sha256(expected_sha256: &str, actual_sha256: &str) -> Result<()> {
    validate_sha256(expected_sha256)?;
    if actual_sha256.len() != 64 || !actual_sha256.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(anyhow!("computed package SHA-256 is malformed"));
    }
    if !expected_sha256.eq_ignore_ascii_case(actual_sha256) {
        return Err(anyhow!(
            "update package SHA-256 mismatch: expected {expected_sha256}, got {actual_sha256}"
        ));
    }
    Ok(())
}

pub fn verify_package_size(expected_size_bytes: u64, actual_size_bytes: u64) -> Result<()> {
    validate_package_size(expected_size_bytes)?;
    if actual_size_bytes != expected_size_bytes {
        return Err(anyhow!(
            "update package size mismatch: expected {expected_size_bytes} bytes, got {actual_size_bytes} bytes"
        ));
    }
    Ok(())
}

fn validate_package_size(size_bytes: u64) -> Result<()> {
    if size_bytes == 0 {
        return Err(anyhow!(
            "update manifest package size must be greater than zero"
        ));
    }
    Ok(())
}

fn validate_sha256(value: &str) -> Result<()> {
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(anyhow!(
            "update manifest package SHA-256 must contain exactly 64 hexadecimal characters"
        ));
    }
    Ok(())
}

pub async fn fetch_manifest(
    client: &reqwest::Client,
    manifest_url: &str,
    if_none_match: Option<&str>,
) -> Result<ManifestFetchResult> {
    let mut request = client.get(manifest_url);
    if let Some(tag) = if_none_match {
        request = request.header(IF_NONE_MATCH, tag);
    }
    let response = request
        .send()
        .await
        .with_context(|| format!("request manifest {manifest_url}"))?;

    if response.status() == reqwest::StatusCode::NOT_MODIFIED {
        return Ok(ManifestFetchResult::NotModified);
    }
    if response.status() == reqwest::StatusCode::NO_CONTENT {
        return Ok(ManifestFetchResult::NoUpdate);
    }
    let mut response = response
        .error_for_status()
        .with_context(|| format!("manifest request failed for {manifest_url}"))?;

    let headers = response.headers().clone();
    if response
        .content_length()
        .is_some_and(|length| length > MAX_MANIFEST_BYTES as u64)
    {
        return Err(anyhow!(
            "signed update manifest exceeds the {MAX_MANIFEST_BYTES} byte limit"
        ));
    }
    let mut manifest_bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .context("read manifest response body")?
    {
        if manifest_bytes.len().saturating_add(chunk.len()) > MAX_MANIFEST_BYTES {
            return Err(anyhow!(
                "signed update manifest exceeds the {MAX_MANIFEST_BYTES} byte limit"
            ));
        }
        manifest_bytes.extend_from_slice(&chunk);
    }
    let signature_b64 = headers
        .get(MANIFEST_SIGNATURE_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow!("missing {MANIFEST_SIGNATURE_HEADER} header"))?;
    let key_id = headers
        .get(MANIFEST_KEY_ID_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let etag = headers
        .get(ETAG)
        .and_then(|value| value.to_str().ok())
        .map(|value| value.to_string());
    let manifest: UpdateManifest =
        serde_json::from_slice(&manifest_bytes).context("parse signed update manifest")?;

    Ok(ManifestFetchResult::Signed(SignedManifest {
        manifest,
        manifest_bytes,
        signature_b64,
        key_id,
        etag,
    }))
}

pub async fn download_file(
    client: &reqwest::Client,
    url: &str,
    destination: &Path,
    expected_size_bytes: u64,
) -> Result<()> {
    validate_package_size(expected_size_bytes)?;
    let _ = std::fs::remove_file(destination);
    let mut response = client
        .get(url)
        .send()
        .await
        .with_context(|| format!("download {url}"))?
        .error_for_status()
        .with_context(|| format!("download failed for {url}"))?;
    if let Some(content_length) = response.content_length() {
        verify_package_size(expected_size_bytes, content_length)
            .context("update package Content-Length verification failed")?;
    }
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    let result = async {
        let mut file = File::create(destination)
            .with_context(|| format!("create {}", destination.display()))?;
        let mut actual_size_bytes = 0_u64;
        while let Some(chunk) = response.chunk().await.context("read package bytes")? {
            actual_size_bytes = actual_size_bytes
                .checked_add(chunk.len() as u64)
                .context("downloaded package size overflow")?;
            if actual_size_bytes > expected_size_bytes {
                return Err(anyhow!(
                    "update package exceeded its signed size of {expected_size_bytes} bytes"
                ));
            }
            file.write_all(&chunk)
                .with_context(|| format!("write {}", destination.display()))?;
        }
        file.flush()
            .with_context(|| format!("flush {}", destination.display()))?;
        verify_package_size(expected_size_bytes, actual_size_bytes)
    }
    .await;
    if result.is_err() {
        let _ = std::fs::remove_file(destination);
    }
    result
}

fn hex_lower(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(&mut out, "{byte:02x}");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIGNED_MANIFEST_FIXTURE: &[u8] = br#"{"product":"worker","version":"1.2.3","package":{"sha256":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}}"#;
    const SIGNING_PUBLIC_KEY_DER_B64: &str = "MIIBCgKCAQEAw8GEsfIA4uJ/3EN3EHBwHMzaCTU78zC5DO1UV7yPI0pIQlrYlEWGf+tnidfcpaT6KvV5b2Ro/0ADThGtZ9sOwL9AYte6sPC/f4+h8ziHKZc2W0ur2ZJrJ10+DYch+5XiRGb4HwoNXgK+P1NbSf4yfM6D+4tFQNVZgPThf0ym8lrD99Xr3Kj6Gff+pUp53MDz/sSGK+sdh1GYMsd10aGzevMc6gK8mvGLNmkAkYupc/olhtTsY7H90puxIkAy4h19vwgS+nTIbI9NJfYNh/BHoIPVGeq0MqiTz+ew7ijpNMH6bH3vtv/fggY/g6YSVY3dUwyn5UYtokzd2wDWVHSkSwIDAQAB";
    const SIGNATURE_B64: &str = "MvkKy7ypyQpTq7/o7W9BTz0aY9sNMfChcbJhdKZ0BthPINHi4lnu8YmW6T4t1eHxnb6xP4AAWPiS4JRcQ/A6Dy2H6PcMGB3R3smO+GLDHhvBC5rD2JyWaW126BT0eogRLR7VaOLtqnvXlmygvoSqlpAcghpWMCxgQi+pqBxEDtxlTRztfeWdZVcjTFyi3MucPrSWP9S/SQhpXcCaMHNgVHwwvCJpQRFoxBj2f2RTyVKqVnPKIeo/foeLZDmR6zSTC37fwocPwB0Cs3hgLH7VFziu80xpShnVrk/tVMHsUQyX7swfxEBGUGx1GmCA1CItNCTQTCXvkZhIfHMnKtazNQ==";
    const OTHER_PUBLIC_KEY_DER_B64: &str = "MIICCgKCAgEAs2czdlg02I7zfciowTMPEKVHQkWm4nY8wpuE7l/w/fDQl0JljwkTZmsROBqaFl0kb/EEoBeduuTTqHV+dJ+/x0zNJtMk/CgAha5POOtjtS298DvOIv81Dp90fYGcxCVGhajtn6EuOiqF98q6zrbBuroNDj+39dU+abgaIlycTlalzJnRt6bQtbfa0aLn9MSnwNhkqunyN3lMvlum/SgJ7ZT7VnN3VCXc6aTb2Dm5wN09qE5D8S4fBLpYYtXKA99mDUbb7PYcRTH/BykwBCEGIuhlONS6gfVPsRAwt/3VBBaGxEVUy5wzWo6MLjwh64lpwM5Jzn6aj48bBz/baKIhUX0yz5xR1ifL44z1CNnL55MAW3t5K07YNqtk9yRbm1gnipezWOAbFS30SHU5X7oj4cI4g8XiYv+N9mXx7YGMRkkdsN7QBsx6lBAXTeDjNEQOy53NpmoYJDtmyZrRV/TrGFfOeG4gjiXsofp3B1uZfKcZ3aKP0TiUa3BjSId+zE93nFrLGGLdkig1/Ckr4rhrlz1Drb030dEPRbM50ABKpwIcPZFW97oqzi4R7/ADZo71bcWp0bQHcG7AHwfjIZWpb36em+B31+NuTJ2t62e/U0RyVH3lLTUPimTVnv/BohlC9Jz2/qVmLP+522+bbhB2CpVsXvE0G68I3PMyhF4uMRECAwEAAQ==";

    fn worker_manifest() -> UpdateManifest {
        UpdateManifest {
            product: "worker".to_string(),
            platform: "linux".to_string(),
            arch: "linux-x64".to_string(),
            channel: "stable".to_string(),
            ring: Some("pilot".to_string()),
            version: "1.2.3".to_string(),
            minimum_supported_version: Some("1.0.0".to_string()),
            severity: "normal".to_string(),
            published_at_utc: "2026-08-28T00:00:00Z".to_string(),
            rollout_percentage: Some(100),
            package: UpdatePackageArtifact {
                file_name: "Talos.Worker.linux-x64.Update.zip".to_string(),
                size_bytes: 123,
                sha256: "a".repeat(64),
            },
            contents: vec!["talos_worker".to_string()],
            requires_restart: true,
            install_mode: "silent".to_string(),
        }
    }

    #[test]
    fn update_base_url_appends_path_exactly_once() {
        assert_eq!(
            normalize_update_base_url(" https://talos.example.test/api/ "),
            Some("https://talos.example.test/api/rmm/updates".to_string())
        );
        assert_eq!(
            normalize_update_base_url("https://updates.example.test/rmm/updates/"),
            Some("https://updates.example.test/rmm/updates".to_string())
        );
    }

    #[test]
    fn update_base_url_rejects_blank_non_http_and_ambiguous_values() {
        for value in [
            "   ",
            "file:///tmp/updates",
            "https://user:secret@talos.example",
            "https://talos.example?tenant=one",
            "https://talos.example#updates",
        ] {
            assert_eq!(normalize_update_base_url(value), None, "accepted {value}");
        }
    }

    #[test]
    fn matching_manifest_signature_is_accepted() {
        let public_key = BASE64_STANDARD
            .decode(SIGNING_PUBLIC_KEY_DER_B64)
            .expect("public fixture is valid base64");
        verify_manifest_signature(&public_key, SIGNED_MANIFEST_FIXTURE, SIGNATURE_B64)
            .expect("matching fixture signature must verify");
    }

    #[test]
    fn tampered_manifest_and_wrong_key_are_rejected() {
        let public_key = BASE64_STANDARD
            .decode(SIGNING_PUBLIC_KEY_DER_B64)
            .expect("public fixture is valid base64");
        let wrong_public_key = BASE64_STANDARD
            .decode(OTHER_PUBLIC_KEY_DER_B64)
            .expect("other public fixture is valid base64");
        let mut tampered = SIGNED_MANIFEST_FIXTURE.to_vec();
        tampered.push(b' ');

        assert!(verify_manifest_signature(&public_key, &tampered, SIGNATURE_B64).is_err());
        assert!(verify_manifest_signature(
            &wrong_public_key,
            SIGNED_MANIFEST_FIXTURE,
            SIGNATURE_B64
        )
        .is_err());
    }

    #[test]
    fn package_digest_must_be_well_formed_and_match() {
        let digest = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        verify_package_sha256(&digest.to_ascii_uppercase(), digest)
            .expect("hex digest comparison is case-insensitive");

        assert!(verify_package_sha256(&"0".repeat(64), digest).is_err());
        assert!(verify_package_sha256("not-a-digest", digest).is_err());
    }

    #[test]
    fn manifest_context_binds_every_artifact_selector_before_download() {
        let expected =
            UpdateManifestExpectation::for_artifact("worker", "linux-x64", "stable", Some("pilot"))
                .expect("expected context");
        validate_manifest_context(&worker_manifest(), &expected).expect("matching manifest");

        let mut mutations: Vec<(&str, Box<dyn Fn(&mut UpdateManifest)>)> = vec![
            (
                "product",
                Box::new(|value| value.product = "supervisor".to_string()),
            ),
            (
                "platform",
                Box::new(|value| value.platform = "windows".to_string()),
            ),
            (
                "architecture",
                Box::new(|value| value.arch = "linux-arm64".to_string()),
            ),
            (
                "channel",
                Box::new(|value| value.channel = "preview".to_string()),
            ),
            ("ring", Box::new(|value| value.ring = None)),
            (
                "install mode",
                Box::new(|value| value.install_mode = "zip".to_string()),
            ),
            (
                "package file name",
                Box::new(|value| {
                    value.package.file_name = "Talos.Supervisor.linux-x64.Update.zip".to_string()
                }),
            ),
            (
                "package size",
                Box::new(|value| value.package.size_bytes = 0),
            ),
        ];
        for (label, mutate) in mutations.drain(..) {
            let mut manifest = worker_manifest();
            mutate(&mut manifest);
            assert!(
                validate_manifest_context(&manifest, &expected).is_err(),
                "accepted mismatched {label}"
            );
        }
    }

    #[test]
    fn expected_context_derives_platform_install_mode_and_exact_package_name() {
        let macos_viewer =
            UpdateManifestExpectation::for_artifact("viewer", "macos-arm64", "stable", None)
                .expect("macOS viewer");
        assert_eq!(macos_viewer.platform, "macos");
        assert_eq!(macos_viewer.install_mode, "pkg");
        assert_eq!(macos_viewer.package_file_name, "Talos.Viewer.macos.pkg");

        let windows_viewer =
            UpdateManifestExpectation::for_artifact("viewer", "x64", "stable", None)
                .expect("Windows viewer");
        assert_eq!(windows_viewer.platform, "windows");
        assert_eq!(windows_viewer.install_mode, "restart");
        assert_eq!(
            windows_viewer.package_file_name,
            "Talos.Viewer.x64.Update.zip"
        );
    }

    #[test]
    fn signed_package_size_must_be_nonzero_and_match_downloaded_bytes() {
        verify_package_size(123, 123).expect("matching size");
        assert!(verify_package_size(0, 0).is_err());
        assert!(verify_package_size(123, 122).is_err());
        assert!(verify_package_size(123, 124).is_err());
    }
}
