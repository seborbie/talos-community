#![cfg_attr(not(target_os = "windows"), allow(dead_code, unused_imports))]

use std::{
    path::{Path, PathBuf},
    time::Duration,
};

#[cfg(target_os = "windows")]
use std::env;
#[cfg(target_os = "windows")]
use std::time::Instant;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
#[cfg(target_os = "windows")]
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::json;
#[cfg(target_os = "windows")]
use tokio::io::AsyncWriteExt;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;
use tracing::{info, warn};
use uuid::Uuid;

const STAGE_ISO_POLL_INTERVAL: Duration = Duration::from_secs(30);
const STAGE_ISO_POLL_RESPONSE_TIMEOUT: Duration = Duration::from_secs(25);
const STAGE_ISO_PROGRESS_INTERVAL: Duration = Duration::from_secs(30);
const STAGE_ISO_CLEANUP_INTERVAL: Duration = Duration::from_secs(60 * 60);

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FeatureUpgradeStageIsoJobsEnvelope {
    pub request_id: String,
    pub jobs: Vec<FeatureUpgradeStageIsoJob>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
pub(crate) struct FeatureUpgradeStageIsoJob {
    pub operation_id: String,
    pub run_id: String,
    pub organization_id: String,
    pub agent_id: String,
    pub source_os: String,
    pub target_product: String,
    pub target_version: String,
    pub target_build_label: String,
    pub retention_seconds: u64,
    pub iso_media: FeatureUpgradeIsoMedia,
    pub download: FeatureUpgradeIsoDownload,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FeatureUpgradeIsoMedia {
    pub id: String,
    pub display_name: String,
    pub os_family: String,
    pub product: String,
    pub version: String,
    pub edition: Option<String>,
    pub architecture: String,
    pub language: Option<String>,
    pub sha256: Option<String>,
    pub size_bytes: Option<u64>,
    pub active: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FeatureUpgradeIsoDownload {
    pub url: String,
    pub expires_at: String,
    #[serde(default)]
    pub method: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FeatureUpgradeStageIsoJobsAvailablePayload {
    pub reason: Option<String>,
    pub requested_by: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FeatureUpgradeStageIsoJobsPollPayload {
    request_id: String,
    limit: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StagedIsoManifest {
    pub(crate) operation_id: String,
    pub(crate) run_id: String,
    pub(crate) organization_id: String,
    pub(crate) agent_id: String,
    pub(crate) iso_media_id: String,
    pub(crate) iso_display_name: String,
    pub(crate) iso_file_name: String,
    pub(crate) size_bytes: Option<u64>,
    pub(crate) sha256: Option<String>,
    pub(crate) staged_at: String,
    pub(crate) expires_at: String,
}

pub(crate) fn start_stage_iso_manager(
    agent_id: String,
    outbound_tx: mpsc::UnboundedSender<Message>,
    mut jobs_rx: mpsc::UnboundedReceiver<FeatureUpgradeStageIsoJobsEnvelope>,
    mut wake_rx: mpsc::UnboundedReceiver<()>,
) {
    start_stage_iso_cleanup_loop(outbound_tx.clone());

    tokio::spawn(async move {
        let mut interval = tokio::time::interval(STAGE_ISO_POLL_INTERVAL);
        interval.tick().await;

        loop {
            tokio::select! {
                _ = interval.tick() => {}
                wake = wake_rx.recv() => {
                    if wake.is_none() {
                        break;
                    }
                }
            }

            let jobs = match poll_stage_iso_jobs(&outbound_tx, &mut jobs_rx).await {
                Ok(jobs) => jobs,
                Err(error) => {
                    warn!(%error, "feature upgrade stage ISO job poll failed");
                    continue;
                }
            };

            for job in jobs {
                if job.agent_id != agent_id {
                    warn!(
                        job_agent_id = %job.agent_id,
                        local_agent_id = %agent_id,
                        "discarding stage ISO job for a different agent"
                    );
                    continue;
                }
                if let Err(error) = run_stage_iso_job(&outbound_tx, job).await {
                    warn!(%error, "feature upgrade stage ISO job failed");
                }
            }
        }
    });
}

pub(crate) fn send_stage_iso_jobs_available_signal(
    wake_tx: &mpsc::UnboundedSender<()>,
    payload: FeatureUpgradeStageIsoJobsAvailablePayload,
) {
    info!(
        reason = ?payload.reason,
        requested_by = ?payload.requested_by,
        "feature upgrade stage ISO jobs available; waking stage manager"
    );
    let _ = wake_tx.send(());
}

async fn poll_stage_iso_jobs(
    outbound_tx: &mpsc::UnboundedSender<Message>,
    jobs_rx: &mut mpsc::UnboundedReceiver<FeatureUpgradeStageIsoJobsEnvelope>,
) -> Result<Vec<FeatureUpgradeStageIsoJob>> {
    let request_id = Uuid::new_v4().to_string();
    send_envelope(
        outbound_tx,
        "feature_upgrade_stage_iso_jobs_poll",
        FeatureUpgradeStageIsoJobsPollPayload {
            request_id: request_id.clone(),
            limit: 1,
        },
    )?;

    tokio::time::timeout(STAGE_ISO_POLL_RESPONSE_TIMEOUT, async {
        while let Some(payload) = jobs_rx.recv().await {
            if payload.request_id == request_id {
                return Ok(payload.jobs);
            }
            warn!(
                expected_request_id = %request_id,
                received_request_id = %payload.request_id,
                "discarding stale feature upgrade stage ISO jobs response"
            );
        }
        Err(anyhow::anyhow!(
            "feature upgrade stage ISO jobs response channel closed"
        ))
    })
    .await
    .context("feature upgrade stage ISO jobs poll timed out")?
}

async fn run_stage_iso_job(
    outbound_tx: &mpsc::UnboundedSender<Message>,
    job: FeatureUpgradeStageIsoJob,
) -> Result<()> {
    send_stage_iso_progress(
        outbound_tx,
        &job,
        "running",
        "requesting_link",
        0,
        None,
        None,
        None,
    )?;

    #[cfg(not(target_os = "windows"))]
    {
        let message = "ISO staging is only supported on Windows agents";
        send_stage_iso_progress(
            outbound_tx,
            &job,
            "failed",
            "failed",
            0,
            job.iso_media.size_bytes,
            None,
            Some(message),
        )?;
        anyhow::bail!(message);
    }

    #[cfg(target_os = "windows")]
    {
        run_stage_iso_job_windows(outbound_tx, job).await
    }
}

#[cfg(target_os = "windows")]
async fn run_stage_iso_job_windows(
    outbound_tx: &mpsc::UnboundedSender<Message>,
    job: FeatureUpgradeStageIsoJob,
) -> Result<()> {
    let stage_root = stage_iso_root();
    let stage_dir = staging_dir_for_operation(&stage_root, &job.operation_id);
    let iso_path = stage_dir.join(iso_file_name(&job.iso_media));

    match run_stage_iso_job_windows_inner(outbound_tx, &job, &stage_root, &stage_dir, &iso_path)
        .await
    {
        Ok(()) => Ok(()),
        Err(error) => {
            let downloaded_bytes = staged_iso_file_size(&iso_path).await.unwrap_or(0);
            let message = error.to_string();
            let _ = send_stage_iso_progress(
                outbound_tx,
                &job,
                "failed",
                "failed",
                downloaded_bytes,
                job.iso_media.size_bytes,
                Some(0.0),
                Some(&message),
            );
            let _ = tokio::fs::remove_file(&iso_path).await;
            Err(error)
        }
    }
}

#[cfg(target_os = "windows")]
async fn run_stage_iso_job_windows_inner(
    outbound_tx: &mpsc::UnboundedSender<Message>,
    job: &FeatureUpgradeStageIsoJob,
    stage_root: &Path,
    stage_dir: &Path,
    iso_path: &Path,
) -> Result<()> {
    let file_name = iso_file_name(&job.iso_media);
    let manifest_path = stage_dir.join("manifest.json");

    tokio::fs::create_dir_all(&stage_dir)
        .await
        .with_context(|| format!("create ISO staging directory {}", stage_dir.display()))?;
    protect_staged_path(stage_root);
    protect_staged_path(&stage_dir);

    send_stage_iso_progress(
        outbound_tx,
        &job,
        "running",
        "downloading",
        0,
        job.iso_media.size_bytes,
        None,
        None,
    )?;

    let downloaded_bytes = download_iso_with_progress(outbound_tx, job, iso_path).await?;
    if let Some(expected) = job.iso_media.size_bytes {
        if downloaded_bytes != expected {
            let message = format!(
                "Downloaded ISO size mismatch: expected {expected} bytes, got {downloaded_bytes} bytes"
            );
            send_stage_iso_progress(
                outbound_tx,
                job,
                "failed",
                "failed",
                downloaded_bytes,
                Some(expected),
                None,
                Some(&message),
            )?;
            anyhow::bail!(message);
        }
    }

    send_stage_iso_progress(
        outbound_tx,
        &job,
        "running",
        "verifying",
        downloaded_bytes,
        Some(downloaded_bytes),
        None,
        None,
    )?;

    if let Some(expected_sha) = job
        .iso_media
        .sha256
        .as_deref()
        .filter(|value| !value.is_empty())
    {
        let iso_path_for_hash = iso_path.to_path_buf();
        let actual_sha = tokio::task::spawn_blocking(move || {
            talos_update_common::sha256_hex_file(&iso_path_for_hash)
        })
        .await
        .context("join ISO sha256 verification")?
        .context("verify ISO sha256")?;
        if !actual_sha.eq_ignore_ascii_case(expected_sha) {
            let message = "Downloaded ISO SHA-256 did not match expected media metadata";
            send_stage_iso_progress(
                outbound_tx,
                &job,
                "failed",
                "failed",
                downloaded_bytes,
                Some(downloaded_bytes),
                None,
                Some(message),
            )?;
            let _ = tokio::fs::remove_file(&iso_path).await;
            anyhow::bail!("{message}");
        }
    }

    protect_staged_path(iso_path);

    let staged_at = Utc::now();
    let expires_at = staged_at + chrono::Duration::seconds(job.retention_seconds as i64);
    let manifest = StagedIsoManifest {
        operation_id: job.operation_id.clone(),
        run_id: job.run_id.clone(),
        organization_id: job.organization_id.clone(),
        agent_id: job.agent_id.clone(),
        iso_media_id: job.iso_media.id.clone(),
        iso_display_name: job.iso_media.display_name.clone(),
        iso_file_name: file_name,
        size_bytes: Some(downloaded_bytes),
        sha256: job.iso_media.sha256.clone(),
        staged_at: staged_at.to_rfc3339(),
        expires_at: expires_at.to_rfc3339(),
    };
    tokio::fs::write(
        &manifest_path,
        serde_json::to_vec_pretty(&manifest).context("serialize staged ISO manifest")?,
    )
    .await
    .context("write staged ISO manifest")?;
    protect_staged_path(&manifest_path);

    send_stage_iso_terminal_progress(outbound_tx, &manifest, "staged", "staged", None)?;
    spawn_stage_iso_cleanup_timer(outbound_tx.clone(), stage_dir.to_path_buf(), manifest);
    Ok(())
}

#[cfg(target_os = "windows")]
async fn download_iso_with_progress(
    outbound_tx: &mpsc::UnboundedSender<Message>,
    job: &FeatureUpgradeStageIsoJob,
    iso_path: &Path,
) -> Result<u64> {
    let client = reqwest::Client::new();
    let response = client
        .get(&job.download.url)
        .send()
        .await
        .context("request ISO download")?
        .error_for_status()
        .context("ISO download returned an error status")?;
    let bytes_total = job
        .iso_media
        .size_bytes
        .or_else(|| response.content_length());
    let mut stream = response.bytes_stream();
    let mut file = tokio::fs::File::create(iso_path)
        .await
        .with_context(|| format!("create staged ISO file {}", iso_path.display()))?;
    let mut downloaded = 0u64;
    let mut last_report_at = Instant::now();
    let mut last_report_bytes = 0u64;

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.context("read ISO download chunk")?;
        file.write_all(&chunk)
            .await
            .context("write ISO download chunk")?;
        downloaded = downloaded.saturating_add(chunk.len() as u64);

        if last_report_at.elapsed() >= STAGE_ISO_PROGRESS_INTERVAL {
            let elapsed = last_report_at.elapsed().as_secs_f64().max(1.0);
            let bytes_per_second = (downloaded.saturating_sub(last_report_bytes)) as f64 / elapsed;
            send_stage_iso_progress(
                outbound_tx,
                job,
                "running",
                "downloading",
                downloaded,
                bytes_total,
                Some(bytes_per_second),
                None,
            )?;
            last_report_at = Instant::now();
            last_report_bytes = downloaded;
        }
    }

    file.flush().await.context("flush staged ISO file")?;
    send_stage_iso_progress(
        outbound_tx,
        job,
        "running",
        "downloading",
        downloaded,
        bytes_total,
        Some(0.0),
        None,
    )?;
    Ok(downloaded)
}

#[cfg(target_os = "windows")]
async fn staged_iso_file_size(path: &Path) -> Option<u64> {
    tokio::fs::metadata(path)
        .await
        .ok()
        .map(|metadata| metadata.len())
}

fn send_stage_iso_progress(
    outbound_tx: &mpsc::UnboundedSender<Message>,
    job: &FeatureUpgradeStageIsoJob,
    status: &str,
    phase: &str,
    bytes_downloaded: u64,
    bytes_total: Option<u64>,
    bytes_per_second: Option<f64>,
    error: Option<&str>,
) -> Result<()> {
    let total = bytes_total.or(job.iso_media.size_bytes);
    let percent = percentage(bytes_downloaded, total, status);
    let payload = json!({
        "operationId": &job.operation_id,
        "runId": &job.run_id,
        "organizationId": &job.organization_id,
        "agentId": &job.agent_id,
        "isoMediaId": &job.iso_media.id,
        "status": status,
        "phase": phase,
        "schemaVersion": 1,
        "eventType": "feature_upgrade.iso.stage.progress",
        "reportedAt": Utc::now().to_rfc3339(),
        "overallPercent": percent,
        "phasePercent": percent,
        "bytesDownloaded": bytes_downloaded,
        "bytesTotal": total,
        "bytesPerSecond": bytes_per_second,
        "retentionSeconds": job.retention_seconds,
        "isoMedia": &job.iso_media,
        "downloadUrlExpiresAt": &job.download.expires_at,
        "downloadMethod": &job.download.method,
        "error": error
    });
    send_envelope(outbound_tx, "feature_upgrade_stage_iso_progress", payload)
}

fn send_stage_iso_terminal_progress(
    outbound_tx: &mpsc::UnboundedSender<Message>,
    manifest: &StagedIsoManifest,
    status: &str,
    phase: &str,
    error: Option<&str>,
) -> Result<()> {
    let cleaned_at = if status == "expired" || status == "deleted" {
        Some(Utc::now().to_rfc3339())
    } else {
        None
    };
    let payload = json!({
        "operationId": &manifest.operation_id,
        "runId": &manifest.run_id,
        "organizationId": &manifest.organization_id,
        "agentId": &manifest.agent_id,
        "isoMediaId": &manifest.iso_media_id,
        "status": status,
        "phase": phase,
        "schemaVersion": 1,
        "eventType": "feature_upgrade.iso.stage.progress",
        "reportedAt": Utc::now().to_rfc3339(),
        "overallPercent": 100,
        "phasePercent": 100,
        "bytesDownloaded": manifest.size_bytes,
        "bytesTotal": manifest.size_bytes,
        "bytesPerSecond": 0,
        "stagedAt": &manifest.staged_at,
        "expiresAt": &manifest.expires_at,
        "cleanedAt": cleaned_at,
        "error": error,
        "evidence": {
            "isoFileName": &manifest.iso_file_name,
            "sha256": &manifest.sha256
        }
    });
    send_envelope(outbound_tx, "feature_upgrade_stage_iso_progress", payload)
}

fn percentage(bytes_downloaded: u64, bytes_total: Option<u64>, status: &str) -> u8 {
    if matches!(status, "staged" | "failed" | "expired" | "deleted") {
        return 100;
    }
    let Some(total) = bytes_total.filter(|value| *value > 0) else {
        return 0;
    };
    ((bytes_downloaded.saturating_mul(100) / total).min(99)) as u8
}

#[cfg(target_os = "windows")]
pub(crate) fn stage_iso_root() -> PathBuf {
    env::var_os("ProgramData")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\ProgramData"))
        .join("Talos")
        .join("FeatureUpgrades")
        .join("StagedIso")
}

pub(crate) fn staging_dir_for_operation(root: &Path, operation_id: &str) -> PathBuf {
    root.join(sanitize_path_component(operation_id))
}

pub(crate) fn iso_file_name(media: &FeatureUpgradeIsoMedia) -> String {
    let base = sanitize_path_component(&media.display_name);
    if base.to_ascii_lowercase().ends_with(".iso") {
        base
    } else {
        format!("{base}.iso")
    }
}

fn sanitize_path_component(value: &str) -> String {
    let sanitized = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.') {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>()
        .trim_matches('.')
        .trim_matches('_')
        .to_string();
    if sanitized.is_empty() {
        "iso".to_string()
    } else {
        sanitized
    }
}

#[cfg(target_os = "windows")]
pub(crate) fn protect_staged_path(path: &Path) {
    if let Err(error) = std::process::Command::new("attrib")
        .arg("+H")
        .arg(path)
        .status()
    {
        warn!(path = %path.display(), %error, "failed to mark staged ISO path hidden");
    }

    let grant_system = if path.is_dir() {
        "SYSTEM:(OI)(CI)F"
    } else {
        "SYSTEM:F"
    };
    let grant_admins = if path.is_dir() {
        "Administrators:(OI)(CI)F"
    } else {
        "Administrators:F"
    };
    if let Err(error) = std::process::Command::new("icacls")
        .arg(path)
        .arg("/inheritance:r")
        .arg("/grant:r")
        .arg(grant_system)
        .arg("/grant:r")
        .arg(grant_admins)
        .status()
    {
        warn!(path = %path.display(), %error, "failed to restrict staged ISO ACLs");
    }
}

fn start_stage_iso_cleanup_loop(outbound_tx: mpsc::UnboundedSender<Message>) {
    tokio::spawn(async move {
        loop {
            if let Err(error) = cleanup_expired_staged_isos(&outbound_tx).await {
                warn!(%error, "expired staged ISO cleanup failed");
            }
            tokio::time::sleep(STAGE_ISO_CLEANUP_INTERVAL).await;
        }
    });
}

async fn cleanup_expired_staged_isos(outbound_tx: &mpsc::UnboundedSender<Message>) -> Result<()> {
    #[cfg(not(target_os = "windows"))]
    {
        let _ = outbound_tx;
        return Ok(());
    }

    #[cfg(target_os = "windows")]
    {
        let root = stage_iso_root();
        let mut entries = match tokio::fs::read_dir(&root).await {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error).context("read staged ISO root"),
        };

        while let Some(entry) = entries
            .next_entry()
            .await
            .context("read staged ISO entry")?
        {
            let path = entry.path();
            let manifest_path = path.join("manifest.json");
            let manifest = match read_manifest(&manifest_path).await {
                Ok(manifest) => manifest,
                Err(error) => {
                    warn!(path = %manifest_path.display(), %error, "skipping invalid staged ISO manifest");
                    continue;
                }
            };
            if manifest_is_expired(&manifest, Utc::now()) {
                delete_staged_iso_dir(&path).await;
                let _ = send_stage_iso_terminal_progress(
                    outbound_tx,
                    &manifest,
                    "expired",
                    "deleted",
                    None,
                );
            }
        }
        Ok(())
    }
}

#[cfg(target_os = "windows")]
pub(crate) async fn read_manifest(path: &Path) -> Result<StagedIsoManifest> {
    let bytes = tokio::fs::read(path)
        .await
        .with_context(|| format!("read staged ISO manifest {}", path.display()))?;
    serde_json::from_slice(&bytes).context("parse staged ISO manifest")
}

pub(crate) fn manifest_is_expired(manifest: &StagedIsoManifest, now: DateTime<Utc>) -> bool {
    DateTime::parse_from_rfc3339(&manifest.expires_at)
        .map(|expires_at| expires_at.with_timezone(&Utc) <= now)
        .unwrap_or(true)
}

#[cfg(target_os = "windows")]
fn spawn_stage_iso_cleanup_timer(
    outbound_tx: mpsc::UnboundedSender<Message>,
    stage_dir: PathBuf,
    manifest: StagedIsoManifest,
) {
    tokio::spawn(async move {
        let sleep_for = DateTime::parse_from_rfc3339(&manifest.expires_at)
            .map(|expires_at| {
                expires_at
                    .with_timezone(&Utc)
                    .signed_duration_since(Utc::now())
                    .to_std()
                    .unwrap_or_else(|_| Duration::from_secs(0))
            })
            .unwrap_or_else(|_| Duration::from_secs(0));
        tokio::time::sleep(sleep_for).await;
        delete_staged_iso_dir(&stage_dir).await;
        let _ =
            send_stage_iso_terminal_progress(&outbound_tx, &manifest, "expired", "deleted", None);
    });
}

#[cfg(target_os = "windows")]
async fn delete_staged_iso_dir(path: &Path) {
    if let Err(error) = tokio::fs::remove_dir_all(path).await {
        if error.kind() != std::io::ErrorKind::NotFound {
            warn!(path = %path.display(), %error, "failed to remove expired staged ISO directory");
        }
    }
}

fn send_envelope<T: Serialize>(
    outbound_tx: &mpsc::UnboundedSender<Message>,
    message_type: &'static str,
    data: T,
) -> Result<()> {
    let text = serde_json::to_string(&json!({
        "type": message_type,
        "data": data
    }))
    .context("serialize feature upgrade stage ISO envelope")?;
    outbound_tx
        .send(Message::Text(text))
        .map_err(|_| anyhow::anyhow!("websocket outbound channel closed"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn staging_dir_uses_operation_under_root() {
        let root = PathBuf::from(r"C:\ProgramData\Talos\FeatureUpgrades\StagedIso");
        let dir = staging_dir_for_operation(&root, "operation:1");
        assert_eq!(dir, root.join("operation_1"));
    }

    #[test]
    fn iso_file_name_sanitizes_display_name() {
        let media = FeatureUpgradeIsoMedia {
            id: "media-1".to_string(),
            display_name: "Windows 11 25H2 / en-GB".to_string(),
            os_family: "windows".to_string(),
            product: "Windows 11".to_string(),
            version: "25H2".to_string(),
            edition: None,
            architecture: "x64".to_string(),
            language: Some("en-GB".to_string()),
            sha256: None,
            size_bytes: None,
            active: true,
        };
        assert_eq!(iso_file_name(&media), "Windows_11_25H2___en-GB.iso");
    }

    #[test]
    fn manifest_expiry_is_inclusive() {
        let manifest = StagedIsoManifest {
            operation_id: "op".to_string(),
            run_id: "run".to_string(),
            organization_id: "org".to_string(),
            agent_id: "agent".to_string(),
            iso_media_id: "media".to_string(),
            iso_display_name: "media".to_string(),
            iso_file_name: "media.iso".to_string(),
            size_bytes: Some(1),
            sha256: None,
            staged_at: "2026-05-25T00:00:00Z".to_string(),
            expires_at: "2026-06-01T00:00:00Z".to_string(),
        };
        assert!(manifest_is_expired(
            &manifest,
            DateTime::parse_from_rfc3339("2026-06-01T00:00:00Z")
                .unwrap()
                .with_timezone(&Utc)
        ));
    }
}
