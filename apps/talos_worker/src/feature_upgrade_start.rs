#![cfg_attr(not(target_os = "windows"), allow(dead_code, unused_imports))]

use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};

#[cfg(target_os = "windows")]
use std::path::{Path, PathBuf};
#[cfg(target_os = "windows")]
use std::time::Instant;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
#[cfg(target_os = "windows")]
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::mpsc;
#[cfg(target_os = "windows")]
use tokio::{io::AsyncWriteExt, process::Command};
use tokio_tungstenite::tungstenite::Message;
use tracing::{info, warn};
use uuid::Uuid;

use crate::collect_and_queue_full_snapshot;
#[cfg(target_os = "windows")]
use crate::feature_upgrade_stage_iso::{
    iso_file_name, manifest_is_expired, protect_staged_path, read_manifest, stage_iso_root,
    staging_dir_for_operation, StagedIsoManifest,
};
use crate::feature_upgrade_stage_iso::{FeatureUpgradeIsoDownload, FeatureUpgradeIsoMedia};

const START_POLL_INTERVAL: Duration = Duration::from_secs(30);
const START_POLL_RESPONSE_TIMEOUT: Duration = Duration::from_secs(25);
const START_PROGRESS_INTERVAL: Duration = Duration::from_secs(30);
const VERIFY_TIMEOUT_HOURS: i64 = 24;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FeatureUpgradeStartJobsEnvelope {
    pub request_id: String,
    pub jobs: Vec<FeatureUpgradeStartJob>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FeatureUpgradeSetupCommand {
    pub id: String,
    pub setup_executable: String,
    #[serde(default)]
    pub arguments: Vec<String>,
    #[serde(default)]
    pub dynamic_update_mode: Option<String>,
    #[serde(default)]
    pub requires_eula_accept: bool,
    #[serde(default)]
    pub image_index_strategy: Option<String>,
    #[serde(default)]
    pub notes: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
pub(crate) struct FeatureUpgradeStartJob {
    pub operation_id: String,
    pub run_id: String,
    pub organization_id: String,
    pub agent_id: String,
    pub source_os: String,
    pub target_product: String,
    pub target_version: String,
    pub target_build_label: String,
    #[serde(default)]
    pub scheduled_for: Option<String>,
    #[serde(default)]
    pub snapshot_request_id: Option<String>,
    pub disk_free_bytes_required: u64,
    pub retention_seconds: u64,
    pub iso_media: FeatureUpgradeIsoMedia,
    pub download: FeatureUpgradeIsoDownload,
    pub setup_command: FeatureUpgradeSetupCommand,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FeatureUpgradeStartJobsAvailablePayload {
    pub reason: Option<String>,
    pub requested_by: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FeatureUpgradeStartJobsPollPayload {
    request_id: String,
    limit: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ActiveUpgradeState {
    operation_id: String,
    run_id: String,
    organization_id: String,
    agent_id: String,
    target_product: String,
    target_version: String,
    target_build_label: String,
    source_os: String,
    iso_media: FeatureUpgradeIsoMedia,
    setup_command: FeatureUpgradeSetupCommand,
    log_dir: String,
    launched_at: String,
    #[serde(default)]
    launched_boot_session_id: Option<String>,
    last_verification_at: Option<String>,
}

pub(crate) fn start_start_manager(
    agent_id: String,
    hostname: String,
    boot_session_id: String,
    outbound_tx: mpsc::UnboundedSender<Message>,
    mut jobs_rx: mpsc::UnboundedReceiver<FeatureUpgradeStartJobsEnvelope>,
    mut wake_rx: mpsc::UnboundedReceiver<()>,
    snapshot_in_progress: Arc<AtomicBool>,
) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(START_POLL_INTERVAL);
        interval.tick().await;

        loop {
            if let Err(error) = resume_pending_upgrade_verifications(
                &outbound_tx,
                &agent_id,
                &hostname,
                &boot_session_id,
                &snapshot_in_progress,
            )
            .await
            {
                warn!(%error, "feature upgrade post-reboot verification scan failed");
            }

            tokio::select! {
                _ = interval.tick() => {}
                wake = wake_rx.recv() => {
                    if wake.is_none() {
                        break;
                    }
                }
            }

            let jobs = match poll_start_jobs(&outbound_tx, &mut jobs_rx).await {
                Ok(jobs) => jobs,
                Err(error) => {
                    warn!(%error, "feature upgrade start job poll failed");
                    continue;
                }
            };

            for job in jobs {
                if job.agent_id != agent_id {
                    warn!(
                        job_agent_id = %job.agent_id,
                        local_agent_id = %agent_id,
                        "discarding feature upgrade start job for a different agent"
                    );
                    continue;
                }
                if let Err(error) = run_start_job(
                    &outbound_tx,
                    &agent_id,
                    &hostname,
                    &boot_session_id,
                    &snapshot_in_progress,
                    job,
                )
                .await
                {
                    warn!(%error, "feature upgrade start job failed");
                }
            }
        }
    });
}

pub(crate) fn send_start_jobs_available_signal(
    wake_tx: &mpsc::UnboundedSender<()>,
    payload: FeatureUpgradeStartJobsAvailablePayload,
) {
    info!(
        reason = ?payload.reason,
        requested_by = ?payload.requested_by,
        "feature upgrade start jobs available; waking start manager"
    );
    let _ = wake_tx.send(());
}

async fn poll_start_jobs(
    outbound_tx: &mpsc::UnboundedSender<Message>,
    jobs_rx: &mut mpsc::UnboundedReceiver<FeatureUpgradeStartJobsEnvelope>,
) -> Result<Vec<FeatureUpgradeStartJob>> {
    let request_id = Uuid::new_v4().to_string();
    send_envelope(
        outbound_tx,
        "feature_upgrade_start_jobs_poll",
        FeatureUpgradeStartJobsPollPayload {
            request_id: request_id.clone(),
            limit: 1,
        },
    )?;

    tokio::time::timeout(START_POLL_RESPONSE_TIMEOUT, async {
        while let Some(payload) = jobs_rx.recv().await {
            if payload.request_id == request_id {
                return Ok(payload.jobs);
            }
            warn!(
                expected_request_id = %request_id,
                received_request_id = %payload.request_id,
                "discarding stale feature upgrade start jobs response"
            );
        }
        Err(anyhow::anyhow!(
            "feature upgrade start jobs response channel closed"
        ))
    })
    .await
    .context("feature upgrade start jobs poll timed out")?
}

async fn run_start_job(
    outbound_tx: &mpsc::UnboundedSender<Message>,
    agent_id: &str,
    hostname: &str,
    boot_session_id: &str,
    snapshot_in_progress: &Arc<AtomicBool>,
    job: FeatureUpgradeStartJob,
) -> Result<()> {
    send_start_progress(
        outbound_tx,
        &job,
        "running",
        "final_checks",
        5,
        None,
        json!({}),
    )?;

    #[cfg(not(target_os = "windows"))]
    {
        let _ = (agent_id, hostname, boot_session_id, snapshot_in_progress);
        let message = "Feature upgrade start is only supported on Windows agents";
        send_start_progress(
            outbound_tx,
            &job,
            "failed",
            "failed",
            100,
            Some(message),
            json!({ "platform": std::env::consts::OS }),
        )?;
        anyhow::bail!(message);
    }

    #[cfg(target_os = "windows")]
    {
        run_start_job_windows(
            outbound_tx,
            agent_id,
            hostname,
            boot_session_id,
            snapshot_in_progress,
            job,
        )
        .await
    }
}

#[cfg(target_os = "windows")]
async fn run_start_job_windows(
    outbound_tx: &mpsc::UnboundedSender<Message>,
    agent_id: &str,
    hostname: &str,
    boot_session_id: &str,
    snapshot_in_progress: &Arc<AtomicBool>,
    job: FeatureUpgradeStartJob,
) -> Result<()> {
    let snapshot_request_id = job
        .snapshot_request_id
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(job.operation_id.as_str())
        .to_string();
    collect_and_queue_full_snapshot(
        outbound_tx,
        agent_id,
        hostname,
        boot_session_id,
        Some(snapshot_request_id),
        snapshot_in_progress,
        "feature_upgrade_start_final_snapshot",
    )
    .await
    .context("collect final feature upgrade snapshot")?;

    let final_gate = collect_final_gate_evidence(
        job.disk_free_bytes_required,
        &job.iso_media,
        &job.target_product,
        &job.target_version,
    )
    .await?;
    if !final_gate.passed {
        let message = final_gate
            .failure_reasons
            .first()
            .cloned()
            .unwrap_or_else(|| "Final feature upgrade checks failed".to_string());
        send_start_progress(
            outbound_tx,
            &job,
            "failed",
            "failed",
            100,
            Some(&message),
            final_gate.to_evidence(),
        )?;
        anyhow::bail!(message);
    }

    send_start_progress(
        outbound_tx,
        &job,
        "running",
        "resolving_iso",
        15,
        None,
        final_gate.to_evidence(),
    )?;

    let iso_path = resolve_or_download_iso(outbound_tx, &job).await?;
    send_start_progress(
        outbound_tx,
        &job,
        "running",
        "mounting_iso",
        55,
        None,
        json!({ "isoPath": iso_path.display().to_string() }),
    )?;
    let mount_drive = mount_iso_and_get_drive(&iso_path).await?;

    let log_dir = active_log_dir(&job.operation_id);
    tokio::fs::create_dir_all(&log_dir)
        .await
        .with_context(|| format!("create feature upgrade log dir {}", log_dir.display()))?;
    protect_staged_path(&log_dir);

    let current_windows = current_windows_edition_info().await.ok();
    let current_edition = current_windows
        .as_ref()
        .and_then(WindowsEditionInfo::edition_for_command);
    let setup_path = expand_template(
        &job.setup_command.setup_executable,
        &mount_drive,
        &log_dir,
        &job.target_version,
        current_edition.as_deref(),
    )?;
    let mut args = expand_args(
        &job.setup_command.arguments,
        &mount_drive,
        &log_dir,
        &job.target_version,
        current_edition.as_deref(),
    )?;
    if should_append_image_index(&job.setup_command) {
        if let Some(index) = resolve_image_index(&mount_drive, current_windows.as_ref()).await {
            args.push("/imageindex".to_string());
            args.push(index);
        }
    }

    let state = ActiveUpgradeState {
        operation_id: job.operation_id.clone(),
        run_id: job.run_id.clone(),
        organization_id: job.organization_id.clone(),
        agent_id: job.agent_id.clone(),
        target_product: job.target_product.clone(),
        target_version: job.target_version.clone(),
        target_build_label: job.target_build_label.clone(),
        source_os: job.source_os.clone(),
        iso_media: job.iso_media.clone(),
        setup_command: job.setup_command.clone(),
        log_dir: log_dir.display().to_string(),
        launched_at: Utc::now().to_rfc3339(),
        launched_boot_session_id: Some(boot_session_id.to_string()),
        last_verification_at: None,
    };
    write_active_upgrade_state(&state).await?;

    send_start_progress(
        outbound_tx,
        &job,
        "running",
        "launching_setup",
        65,
        None,
        json!({
            "setupPath": setup_path,
            "arguments": args,
            "logDir": log_dir.display().to_string()
        }),
    )?;

    let mut child = match Command::new(&setup_path).args(&args).spawn() {
        Ok(child) => child,
        Err(error) => {
            let _ = tokio::fs::remove_file(active_state_path(&job.operation_id)).await;
            return Err(error).with_context(|| format!("launch Windows Setup from {setup_path}"));
        }
    };

    send_start_progress(
        outbound_tx,
        &job,
        "running",
        "setup_running",
        70,
        None,
        json!({ "setupPath": setup_path, "logDir": log_dir.display().to_string() }),
    )?;

    let heartbeat_stop = Arc::new(AtomicBool::new(false));
    let heartbeat_stop_for_task = heartbeat_stop.clone();
    let heartbeat_tx = outbound_tx.clone();
    let heartbeat_job = job.clone();
    let heartbeat_log_dir = log_dir.clone();
    let heartbeat = tokio::spawn(async move {
        loop {
            tokio::time::sleep(START_PROGRESS_INTERVAL).await;
            if heartbeat_stop_for_task.load(Ordering::SeqCst) {
                break;
            }
            let _ = send_start_progress(
                &heartbeat_tx,
                &heartbeat_job,
                "running",
                "setup_running",
                72,
                None,
                json!({ "logDir": heartbeat_log_dir.display().to_string() }),
            );
        }
    });

    let status = child
        .wait()
        .await
        .context("wait for Windows Setup process")?;
    heartbeat_stop.store(true, Ordering::SeqCst);
    heartbeat.abort();

    let exit_code = status.code().unwrap_or(-1);
    if matches!(exit_code, 0 | 1641 | 3010) {
        send_start_progress(
            outbound_tx,
            &job,
            "awaiting_reboot",
            "awaiting_reboot",
            80,
            None,
            json!({ "setupExitCode": exit_code, "logDir": log_dir.display().to_string() }),
        )?;
        Ok(())
    } else {
        let message = setup_exit_message(exit_code);
        let evidence = json!({
            "setupExitCode": exit_code,
            "setupExitCodeHex": setup_exit_code_hex(exit_code),
            "logDir": log_dir.display().to_string()
        });
        send_start_progress(
            outbound_tx,
            &job,
            "failed",
            "failed",
            100,
            Some(&message),
            evidence,
        )?;
        let _ = tokio::fs::remove_file(active_state_path(&job.operation_id)).await;
        anyhow::bail!(message)
    }
}

#[cfg(target_os = "windows")]
async fn resolve_or_download_iso(
    outbound_tx: &mpsc::UnboundedSender<Message>,
    job: &FeatureUpgradeStartJob,
) -> Result<PathBuf> {
    if let Some(path) = find_valid_staged_iso(&job.iso_media).await? {
        send_start_progress(
            outbound_tx,
            job,
            "running",
            "resolving_iso",
            25,
            None,
            json!({ "stagedIsoPath": path.display().to_string(), "usedStagedIso": true }),
        )?;
        return Ok(path);
    }

    let stage_root = stage_iso_root();
    let stage_dir = staging_dir_for_operation(&stage_root, &job.operation_id);
    let iso_path = stage_dir.join(iso_file_name(&job.iso_media));
    tokio::fs::create_dir_all(&stage_dir)
        .await
        .with_context(|| {
            format!(
                "create transient ISO staging directory {}",
                stage_dir.display()
            )
        })?;
    protect_staged_path(&stage_root);
    protect_staged_path(&stage_dir);

    send_start_progress(
        outbound_tx,
        job,
        "running",
        "downloading_iso",
        25,
        None,
        json!({ "usedStagedIso": false }),
    )?;
    let downloaded_bytes = download_iso_with_progress(outbound_tx, job, &iso_path).await?;
    if let Some(expected) = job.iso_media.size_bytes {
        if downloaded_bytes != expected {
            let message = format!(
                "Downloaded ISO size mismatch: expected {expected} bytes, got {downloaded_bytes} bytes"
            );
            send_start_progress(
                outbound_tx,
                job,
                "failed",
                "failed",
                100,
                Some(&message),
                json!({ "downloadedBytes": downloaded_bytes, "expectedBytes": expected }),
            )?;
            anyhow::bail!(message);
        }
    }

    send_start_progress(
        outbound_tx,
        job,
        "running",
        "verifying_iso",
        45,
        None,
        json!({ "downloadedBytes": downloaded_bytes }),
    )?;
    verify_iso_hash(job, &iso_path).await?;
    protect_staged_path(&iso_path);
    write_staged_manifest(job, &stage_dir, downloaded_bytes).await?;
    Ok(iso_path)
}

#[cfg(target_os = "windows")]
async fn find_valid_staged_iso(media: &FeatureUpgradeIsoMedia) -> Result<Option<PathBuf>> {
    let root = stage_iso_root();
    let mut entries = match tokio::fs::read_dir(&root).await {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).context("read staged ISO root"),
    };
    while let Some(entry) = entries
        .next_entry()
        .await
        .context("read staged ISO entry")?
    {
        let dir = entry.path();
        let manifest_path = dir.join("manifest.json");
        let manifest = match read_manifest(&manifest_path).await {
            Ok(manifest) => manifest,
            Err(_) => continue,
        };
        if manifest.iso_media_id != media.id || manifest_is_expired(&manifest, Utc::now()) {
            continue;
        }
        let iso_path = dir.join(&manifest.iso_file_name);
        let metadata = match tokio::fs::metadata(&iso_path).await {
            Ok(metadata) => metadata,
            Err(_) => continue,
        };
        if let Some(expected) = media.size_bytes.or(manifest.size_bytes) {
            if metadata.len() != expected {
                continue;
            }
        }
        if let Some(expected_sha) = media.sha256.as_deref().filter(|value| !value.is_empty()) {
            let path_for_hash = iso_path.clone();
            let actual_sha = tokio::task::spawn_blocking(move || {
                talos_update_common::sha256_hex_file(&path_for_hash)
            })
            .await
            .context("join staged ISO sha256 verification")?
            .context("verify staged ISO sha256")?;
            if !actual_sha.eq_ignore_ascii_case(expected_sha) {
                continue;
            }
        }
        return Ok(Some(iso_path));
    }
    Ok(None)
}

#[cfg(target_os = "windows")]
async fn download_iso_with_progress(
    outbound_tx: &mpsc::UnboundedSender<Message>,
    job: &FeatureUpgradeStartJob,
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

        if last_report_at.elapsed() >= START_PROGRESS_INTERVAL {
            let elapsed = last_report_at.elapsed().as_secs_f64().max(1.0);
            let bytes_per_second = (downloaded.saturating_sub(last_report_bytes)) as f64 / elapsed;
            send_start_progress(
                outbound_tx,
                job,
                "running",
                "downloading_iso",
                percentage(downloaded, bytes_total, 25, 44),
                None,
                json!({
                    "bytesDownloaded": downloaded,
                    "bytesTotal": bytes_total,
                    "bytesPerSecond": bytes_per_second
                }),
            )?;
            last_report_at = Instant::now();
            last_report_bytes = downloaded;
        }
    }

    file.flush().await.context("flush staged ISO file")?;
    Ok(downloaded)
}

#[cfg(target_os = "windows")]
async fn verify_iso_hash(job: &FeatureUpgradeStartJob, iso_path: &Path) -> Result<()> {
    let Some(expected_sha) = job
        .iso_media
        .sha256
        .as_deref()
        .filter(|value| !value.is_empty())
    else {
        return Ok(());
    };
    let iso_path_for_hash = iso_path.to_path_buf();
    let actual_sha = tokio::task::spawn_blocking(move || {
        talos_update_common::sha256_hex_file(&iso_path_for_hash)
    })
    .await
    .context("join ISO sha256 verification")?
    .context("verify ISO sha256")?;
    if !actual_sha.eq_ignore_ascii_case(expected_sha) {
        anyhow::bail!("Downloaded ISO SHA-256 did not match expected media metadata");
    }
    Ok(())
}

#[cfg(target_os = "windows")]
async fn write_staged_manifest(
    job: &FeatureUpgradeStartJob,
    stage_dir: &Path,
    downloaded_bytes: u64,
) -> Result<()> {
    let staged_at = Utc::now();
    let expires_at = staged_at + chrono::Duration::seconds(job.retention_seconds as i64);
    let manifest = StagedIsoManifest {
        operation_id: job.operation_id.clone(),
        run_id: job.run_id.clone(),
        organization_id: job.organization_id.clone(),
        agent_id: job.agent_id.clone(),
        iso_media_id: job.iso_media.id.clone(),
        iso_display_name: job.iso_media.display_name.clone(),
        iso_file_name: iso_file_name(&job.iso_media),
        size_bytes: Some(downloaded_bytes),
        sha256: job.iso_media.sha256.clone(),
        staged_at: staged_at.to_rfc3339(),
        expires_at: expires_at.to_rfc3339(),
    };
    let manifest_path = stage_dir.join("manifest.json");
    tokio::fs::write(
        &manifest_path,
        serde_json::to_vec_pretty(&manifest).context("serialize transient staged ISO manifest")?,
    )
    .await
    .context("write transient staged ISO manifest")?;
    protect_staged_path(&manifest_path);
    Ok(())
}

#[cfg(target_os = "windows")]
#[derive(Debug, Clone)]
struct FinalGateEvidence {
    passed: bool,
    free_bytes_required: u64,
    system_drive: Option<String>,
    system_drive_free_bytes: Option<u64>,
    system_default_ui_language: Option<String>,
    iso_language: Option<String>,
    bitlocker_statuses: Vec<Value>,
    failure_reasons: Vec<String>,
}

#[cfg(target_os = "windows")]
impl FinalGateEvidence {
    fn to_evidence(&self) -> Value {
        json!({
            "finalChecks": {
                "passed": self.passed,
                "freeBytesRequired": self.free_bytes_required,
                "systemDrive": self.system_drive,
                "systemDriveFreeBytes": self.system_drive_free_bytes,
                "systemDefaultUiLanguage": self.system_default_ui_language,
                "isoLanguage": self.iso_language,
                "bitlockerStatuses": self.bitlocker_statuses,
                "failureReasons": self.failure_reasons
            }
        })
    }
}

#[cfg(target_os = "windows")]
async fn collect_final_gate_evidence(
    required_free_bytes: u64,
    iso_media: &FeatureUpgradeIsoMedia,
    target_product: &str,
    target_version: &str,
) -> Result<FinalGateEvidence> {
    let script = r#"
$ErrorActionPreference = 'Stop'
$systemDrive = (Get-CimInstance Win32_OperatingSystem).SystemDrive
$logical = Get-CimInstance Win32_LogicalDisk -Filter ("DeviceID='" + $systemDrive + "'")
$bitlocker = @()
$bitlockerError = $null
try {
  $bitlocker = Get-CimInstance -Namespace 'root\CIMV2\Security\MicrosoftVolumeEncryption' -ClassName Win32_EncryptableVolume |
    Select-Object DriveLetter, ProtectionStatus, ConversionStatus
} catch {
  $bitlockerError = $_.Exception.Message
}
$systemDefaultUiLanguage = $null
try {
  Add-Type -Namespace Talos -Name NativeLocale -MemberDefinition @'
[System.Runtime.InteropServices.DllImport("kernel32.dll")]
public static extern ushort GetSystemDefaultUILanguage();
'@ -ErrorAction SilentlyContinue
  $langId = [Talos.NativeLocale]::GetSystemDefaultUILanguage()
  if ($langId -gt 0) {
    $systemDefaultUiLanguage = [System.Globalization.CultureInfo]::GetCultureInfo([int]$langId).Name
  }
} catch {}
if (-not $systemDefaultUiLanguage) {
  try { $systemDefaultUiLanguage = [System.Globalization.CultureInfo]::InstalledUICulture.Name } catch {}
}
[pscustomobject]@{
  systemDrive = $systemDrive
  freeBytes = [Int64]$logical.FreeSpace
  systemDefaultUiLanguage = $systemDefaultUiLanguage
  bitlocker = $bitlocker
  bitlockerError = $bitlockerError
} | ConvertTo-Json -Depth 6 -Compress
"#;
    let output = Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            script,
        ])
        .output()
        .await
        .context("collect final disk and BitLocker checks")?;
    if !output.status.success() {
        anyhow::bail!(
            "final check command failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let value: Value = serde_json::from_slice(&output.stdout).context("parse final check JSON")?;
    let system_drive = value
        .get("systemDrive")
        .and_then(Value::as_str)
        .map(str::to_string);
    let free_bytes = value.get("freeBytes").and_then(Value::as_u64);
    let bitlocker_statuses = arrayish(value.get("bitlocker"))
        .cloned()
        .collect::<Vec<_>>();
    let system_default_ui_language = value
        .get("systemDefaultUiLanguage")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string);
    let iso_language = iso_media
        .language
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string);
    let mut failure_reasons = Vec::new();
    if free_bytes.unwrap_or(0) < required_free_bytes {
        failure_reasons.push(format!(
            "System drive has {} bytes free; {} bytes are required",
            free_bytes.unwrap_or(0),
            required_free_bytes
        ));
    }
    if bitlocker_statuses
        .iter()
        .any(bitlocker_protected_or_unknown)
    {
        failure_reasons.push(
            "BitLocker protection is enabled or unknown; suspend BitLocker before upgrade"
                .to_string(),
        );
    }
    if windows11_upgrade_requires_exact_language(target_product, target_version) {
        match (iso_language.as_deref(), system_default_ui_language.as_deref()) {
            (Some(media_language), Some(device_language))
                if locales_match_exactly(media_language, device_language) => {}
            (Some(media_language), Some(device_language)) => failure_reasons.push(format!(
                "Selected ISO language {media_language} does not match device system default UI language {device_language}; Windows 11 in-place upgrades require matching media language"
            )),
            (Some(media_language), None) => failure_reasons.push(format!(
                "Could not verify device system default UI language for ISO language {media_language}"
            )),
            _ => {}
        }
    }

    Ok(FinalGateEvidence {
        passed: failure_reasons.is_empty(),
        free_bytes_required: required_free_bytes,
        system_drive,
        system_drive_free_bytes: free_bytes,
        system_default_ui_language,
        iso_language,
        bitlocker_statuses,
        failure_reasons,
    })
}

#[cfg(target_os = "windows")]
fn arrayish(value: Option<&Value>) -> Box<dyn Iterator<Item = &Value> + '_> {
    match value {
        Some(Value::Array(items)) => Box::new(items.iter()),
        Some(Value::Null) | None => Box::new(std::iter::empty()),
        Some(item) => Box::new(std::iter::once(item)),
    }
}

#[cfg(target_os = "windows")]
fn bitlocker_protected_or_unknown(value: &Value) -> bool {
    let record = value.as_object();
    let status = record
        .and_then(|item| item.get("ProtectionStatus"))
        .or_else(|| record.and_then(|item| item.get("protectionStatus")));
    match status {
        Some(Value::Number(number)) => number.as_i64().map(|value| value != 0).unwrap_or(true),
        Some(Value::String(text)) => {
            let lower = text.to_ascii_lowercase();
            lower.contains("on") || lower.contains("protected") || lower.contains("unknown")
        }
        Some(_) => true,
        None => false,
    }
}

#[cfg(target_os = "windows")]
fn windows11_upgrade_requires_exact_language(target_product: &str, target_version: &str) -> bool {
    let product = target_product.to_ascii_lowercase();
    if !product.contains("windows 11") {
        return false;
    }
    let version = target_version.to_ascii_lowercase();
    if let Some(year_text) = version.strip_suffix("h2") {
        return year_text
            .parse::<u32>()
            .map(|year| year >= 22)
            .unwrap_or(true);
    }
    true
}

#[cfg(target_os = "windows")]
fn locales_match_exactly(left: &str, right: &str) -> bool {
    normalize_locale_for_compare(left) == normalize_locale_for_compare(right)
}

#[cfg(target_os = "windows")]
fn normalize_locale_for_compare(value: &str) -> String {
    value.trim().replace('_', "-").to_ascii_lowercase()
}

#[cfg(target_os = "windows")]
async fn mount_iso_and_get_drive(iso_path: &Path) -> Result<String> {
    let script = format!(
        r#"
$ErrorActionPreference = 'Stop'
$imagePath = {}
$image = Mount-DiskImage -ImagePath $imagePath -PassThru
Start-Sleep -Seconds 2
$volume = $image | Get-Volume
if (-not $volume -or -not $volume.DriveLetter) {{
  $volume = Get-DiskImage -ImagePath $imagePath | Get-Volume
}}
if (-not $volume -or -not $volume.DriveLetter) {{
  throw 'Mounted ISO did not expose a drive letter'
}}
[pscustomobject]@{{ drive = ($volume.DriveLetter + ':') }} | ConvertTo-Json -Compress
"#,
        powershell_quote(&iso_path.display().to_string())
    );
    let output = Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            &script,
        ])
        .output()
        .await
        .context("mount Windows setup ISO")?;
    if !output.status.success() {
        anyhow::bail!(
            "mount ISO failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let value: Value =
        serde_json::from_slice(&output.stdout).context("parse mounted ISO drive JSON")?;
    let drive = value
        .get("drive")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| anyhow::anyhow!("mounted ISO drive letter missing"))?;
    Ok(drive.to_string())
}

#[cfg(target_os = "windows")]
fn expand_template(
    template: &str,
    mount_drive: &str,
    log_dir: &Path,
    target_version: &str,
    current_edition: Option<&str>,
) -> Result<String> {
    let mut expanded = template
        .replace("{mount_drive}", mount_drive)
        .replace("{log_dir}", &log_dir.display().to_string());
    if expanded.contains("{target_server_gvlk}") {
        let key = server_gvlk_for_target(target_version, current_edition)
            .ok_or_else(|| anyhow::anyhow!("unable to resolve target Server GVLK product key"))?;
        expanded = expanded.replace("{target_server_gvlk}", key);
    }
    Ok(expanded)
}

#[cfg(target_os = "windows")]
fn expand_args(
    args: &[String],
    mount_drive: &str,
    log_dir: &Path,
    target_version: &str,
    current_edition: Option<&str>,
) -> Result<Vec<String>> {
    args.iter()
        .map(|arg| expand_template(arg, mount_drive, log_dir, target_version, current_edition))
        .collect()
}

#[cfg(target_os = "windows")]
fn server_gvlk_for_target(
    target_version: &str,
    current_edition: Option<&str>,
) -> Option<&'static str> {
    let target = target_version.to_ascii_lowercase();
    if !target.contains("2025") {
        return None;
    }
    let edition = current_edition.unwrap_or_default().to_ascii_lowercase();
    if edition.contains("azure") {
        Some("XGN3F-F394H-FD2MY-PP6FD-8MCRC")
    } else if edition.contains("datacenter") {
        Some("D764K-2NDRG-47T6Q-P8T8W-YP6DF")
    } else if edition.contains("standard") {
        Some("TVRH6-WHNXV-R9WG3-9XRFY-MY832")
    } else {
        None
    }
}

#[cfg(target_os = "windows")]
fn should_append_image_index(command: &FeatureUpgradeSetupCommand) -> bool {
    command
        .image_index_strategy
        .as_deref()
        .map(|value| value.eq_ignore_ascii_case("auto_match_current_edition"))
        .unwrap_or(false)
}

#[cfg(target_os = "windows")]
async fn resolve_image_index(
    mount_drive: &str,
    current_info: Option<&WindowsEditionInfo>,
) -> Option<String> {
    let owned_info;
    let current_info = match current_info {
        Some(value) => value,
        None => {
            owned_info = current_windows_edition_info().await.ok()?;
            &owned_info
        }
    };
    let image_path = windows_image_path(mount_drive).await?;
    let output = Command::new("dism.exe")
        .args(["/English", "/Get-WimInfo"])
        .arg(format!("/WimFile:{image_path}"))
        .output()
        .await
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    parse_matching_image_index(&text, current_info)
}

#[cfg(target_os = "windows")]
#[derive(Debug, Clone, PartialEq, Eq)]
struct WindowsEditionInfo {
    edition_id: Option<String>,
    product_name: Option<String>,
    installation_type: Option<String>,
}

#[cfg(target_os = "windows")]
impl WindowsEditionInfo {
    fn edition_for_command(&self) -> Option<String> {
        self.edition_id
            .as_deref()
            .or(self.product_name.as_deref())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
    }

    fn match_text(&self) -> String {
        [
            self.edition_id.as_deref(),
            self.product_name.as_deref(),
            self.installation_type.as_deref(),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" ")
    }
}

#[cfg(target_os = "windows")]
fn json_string_field(value: &Value, field: &str) -> Option<String> {
    value
        .get(field)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

#[cfg(target_os = "windows")]
async fn current_windows_edition_info() -> Result<WindowsEditionInfo> {
    let script = r#"
$ErrorActionPreference = 'Stop'
$cv = Get-ItemProperty 'HKLM:\SOFTWARE\Microsoft\Windows NT\CurrentVersion'
[pscustomobject]@{
  editionId = $cv.EditionID
  productName = $cv.ProductName
  installationType = $cv.InstallationType
} | ConvertTo-Json -Compress
"#;
    let output = Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            script,
        ])
        .output()
        .await
        .context("collect current Windows edition")?;
    if !output.status.success() {
        anyhow::bail!(
            "current edition command failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let value: Value =
        serde_json::from_slice(&output.stdout).context("parse Windows edition JSON")?;
    let info = WindowsEditionInfo {
        edition_id: json_string_field(&value, "editionId"),
        product_name: json_string_field(&value, "productName"),
        installation_type: json_string_field(&value, "installationType"),
    };
    if info.edition_for_command().is_none() {
        anyhow::bail!("Windows edition was not reported");
    }
    Ok(info)
}

#[cfg(target_os = "windows")]
async fn windows_image_path(mount_drive: &str) -> Option<String> {
    let drive = mount_drive.trim_end_matches('\\');
    let wim = format!(r"{drive}\sources\install.wim");
    if tokio::fs::metadata(&wim).await.is_ok() {
        return Some(wim);
    }
    let esd = format!(r"{drive}\sources\install.esd");
    if tokio::fs::metadata(&esd).await.is_ok() {
        return Some(esd);
    }
    None
}

#[cfg(target_os = "windows")]
fn parse_matching_image_index(
    dism_output: &str,
    current_info: &WindowsEditionInfo,
) -> Option<String> {
    let edition_tokens = edition_match_tokens(&current_info.match_text());
    if edition_tokens.is_empty() {
        return None;
    }
    let server_installation = server_installation_filter(current_info);
    let mut current_index: Option<String> = None;
    let mut current_text = String::new();
    for line in dism_output.lines() {
        let trimmed = line.trim();
        if let Some(index) = trimmed.strip_prefix("Index :") {
            if image_block_matches(&current_text, &edition_tokens, server_installation) {
                return current_index;
            }
            current_index = Some(index.trim().to_string());
            current_text.clear();
        } else if trimmed.starts_with("Name :") || trimmed.starts_with("Description :") {
            current_text.push(' ');
            current_text.push_str(trimmed);
        }
    }
    if image_block_matches(&current_text, &edition_tokens, server_installation) {
        return current_index;
    }
    None
}

#[cfg(target_os = "windows")]
fn edition_match_tokens(value: &str) -> Vec<String> {
    let normalized = value.to_ascii_lowercase();
    let mut tokens = Vec::new();
    if normalized.contains("datacenter") {
        tokens.push("datacenter".to_string());
    }
    if normalized.contains("standard") {
        tokens.push("standard".to_string());
    }
    if normalized.contains("enterprise") {
        tokens.push("enterprise".to_string());
    }
    if normalized.contains("education") {
        tokens.push("education".to_string());
    }
    if normalized.contains("professional") || normalized == "pro" || normalized.contains(" pro") {
        tokens.push("pro".to_string());
        tokens.push("professional".to_string());
    }
    if normalized.contains("home") {
        tokens.push("home".to_string());
    }
    tokens
}

#[cfg(target_os = "windows")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ServerInstallationFilter {
    Core,
    DesktopExperience,
}

#[cfg(target_os = "windows")]
fn server_installation_filter(info: &WindowsEditionInfo) -> Option<ServerInstallationFilter> {
    let product = info
        .product_name
        .as_deref()
        .unwrap_or_default()
        .to_ascii_lowercase();
    let edition = info
        .edition_id
        .as_deref()
        .unwrap_or_default()
        .to_ascii_lowercase();
    let installation_type = info
        .installation_type
        .as_deref()
        .unwrap_or_default()
        .to_ascii_lowercase();
    let is_server = product.contains("windows server")
        || edition.starts_with("server")
        || installation_type.contains("server");
    if !is_server {
        return None;
    }
    if installation_type.contains("core") || product.contains("core") || edition.contains("core") {
        return Some(ServerInstallationFilter::Core);
    }
    if installation_type == "server"
        || installation_type.contains("complete")
        || product.contains("windows server")
    {
        return Some(ServerInstallationFilter::DesktopExperience);
    }
    None
}

#[cfg(target_os = "windows")]
fn image_block_matches(
    block: &str,
    tokens: &[String],
    server_installation: Option<ServerInstallationFilter>,
) -> bool {
    let normalized = block.to_ascii_lowercase();
    if !tokens.iter().any(|token| normalized.contains(token)) {
        return false;
    }
    match server_installation {
        Some(ServerInstallationFilter::DesktopExperience)
            if normalized.contains("windows server") =>
        {
            normalized.contains("desktop experience")
        }
        Some(ServerInstallationFilter::Core) if normalized.contains("windows server") => {
            !normalized.contains("desktop experience")
        }
        _ => true,
    }
}

#[cfg(target_os = "windows")]
fn active_root() -> PathBuf {
    std::env::var_os("ProgramData")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\ProgramData"))
        .join("Talos")
        .join("FeatureUpgrades")
        .join("Active")
}

#[cfg(target_os = "windows")]
fn active_state_path(operation_id: &str) -> PathBuf {
    active_root().join(format!("{}.json", sanitize_file_component(operation_id)))
}

#[cfg(target_os = "windows")]
fn active_log_dir(operation_id: &str) -> PathBuf {
    std::env::var_os("ProgramData")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\ProgramData"))
        .join("Talos")
        .join("FeatureUpgrades")
        .join("Logs")
        .join(sanitize_file_component(operation_id))
}

#[cfg(target_os = "windows")]
async fn write_active_upgrade_state(state: &ActiveUpgradeState) -> Result<()> {
    let root = active_root();
    tokio::fs::create_dir_all(&root)
        .await
        .with_context(|| format!("create active feature upgrade root {}", root.display()))?;
    protect_staged_path(&root);
    let path = active_state_path(&state.operation_id);
    tokio::fs::write(
        &path,
        serde_json::to_vec_pretty(state).context("serialize active feature upgrade state")?,
    )
    .await
    .with_context(|| format!("write active feature upgrade state {}", path.display()))?;
    protect_staged_path(&path);
    Ok(())
}

async fn resume_pending_upgrade_verifications(
    outbound_tx: &mpsc::UnboundedSender<Message>,
    agent_id: &str,
    hostname: &str,
    boot_session_id: &str,
    snapshot_in_progress: &Arc<AtomicBool>,
) -> Result<()> {
    #[cfg(not(target_os = "windows"))]
    {
        let _ = (
            outbound_tx,
            agent_id,
            hostname,
            boot_session_id,
            snapshot_in_progress,
        );
        Ok(())
    }

    #[cfg(target_os = "windows")]
    {
        let root = active_root();
        let mut entries = match tokio::fs::read_dir(&root).await {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error).context("read active feature upgrade root"),
        };
        while let Some(entry) = entries
            .next_entry()
            .await
            .context("read active upgrade entry")?
        {
            let path = entry.path();
            if path.extension().and_then(|value| value.to_str()) != Some("json") {
                continue;
            }
            let bytes = match tokio::fs::read(&path).await {
                Ok(bytes) => bytes,
                Err(error) => {
                    warn!(path = %path.display(), %error, "failed to read active feature upgrade state");
                    continue;
                }
            };
            let mut state: ActiveUpgradeState = match serde_json::from_slice(&bytes) {
                Ok(state) => state,
                Err(error) => {
                    warn!(path = %path.display(), %error, "failed to parse active feature upgrade state");
                    continue;
                }
            };
            if state.agent_id != agent_id {
                continue;
            }

            let job = job_from_state(&state);
            if state.launched_boot_session_id.as_deref() == Some(boot_session_id) {
                send_start_progress(
                    outbound_tx,
                    &job,
                    "awaiting_reboot",
                    "awaiting_reboot",
                    80,
                    None,
                    json!({ "logDir": &state.log_dir }),
                )?;
                continue;
            }

            send_start_progress(
                outbound_tx,
                &job,
                "verifying",
                "post_reboot_verifying",
                90,
                None,
                json!({ "logDir": &state.log_dir }),
            )?;
            let snapshot_id = format!("{}-post-reboot", state.operation_id);
            let _ = collect_and_queue_full_snapshot(
                outbound_tx,
                agent_id,
                hostname,
                boot_session_id,
                Some(snapshot_id),
                snapshot_in_progress,
                "feature_upgrade_post_reboot",
            )
            .await;

            let os_evidence = collect_os_version_evidence().await?;
            if os_matches_target(&os_evidence, &state) {
                send_start_progress(
                    outbound_tx,
                    &job,
                    "succeeded",
                    "completed",
                    100,
                    None,
                    json!({ "os": os_evidence, "logDir": &state.log_dir }),
                )?;
                let _ = tokio::fs::remove_file(&path).await;
                continue;
            }

            let launched_at = DateTime::parse_from_rfc3339(&state.launched_at)
                .map(|value| value.with_timezone(&Utc))
                .unwrap_or_else(|_| Utc::now());
            if Utc::now().signed_duration_since(launched_at).num_hours() >= VERIFY_TIMEOUT_HOURS {
                let message =
                    "Windows version did not match the feature upgrade target within 24 hours";
                send_start_progress(
                    outbound_tx,
                    &job,
                    "failed",
                    "failed",
                    100,
                    Some(message),
                    json!({ "os": os_evidence, "logDir": &state.log_dir }),
                )?;
                let _ = tokio::fs::remove_file(&path).await;
            } else {
                state.last_verification_at = Some(Utc::now().to_rfc3339());
                let _ = write_active_upgrade_state(&state).await;
            }
        }
        Ok(())
    }
}

#[cfg(target_os = "windows")]
fn job_from_state(state: &ActiveUpgradeState) -> FeatureUpgradeStartJob {
    FeatureUpgradeStartJob {
        operation_id: state.operation_id.clone(),
        run_id: state.run_id.clone(),
        organization_id: state.organization_id.clone(),
        agent_id: state.agent_id.clone(),
        source_os: state.source_os.clone(),
        target_product: state.target_product.clone(),
        target_version: state.target_version.clone(),
        target_build_label: state.target_build_label.clone(),
        scheduled_for: None,
        snapshot_request_id: Some(state.operation_id.clone()),
        disk_free_bytes_required: 0,
        retention_seconds: 0,
        iso_media: state.iso_media.clone(),
        download: FeatureUpgradeIsoDownload {
            url: String::new(),
            expires_at: String::new(),
            method: None,
        },
        setup_command: state.setup_command.clone(),
    }
}

#[cfg(target_os = "windows")]
async fn collect_os_version_evidence() -> Result<Value> {
    let script = r#"
$ErrorActionPreference = 'Stop'
$os = Get-CimInstance Win32_OperatingSystem
$cv = Get-ItemProperty 'HKLM:\SOFTWARE\Microsoft\Windows NT\CurrentVersion'
[pscustomobject]@{
  caption = $os.Caption
  version = $os.Version
  buildNumber = $os.BuildNumber
  productName = $cv.ProductName
  displayVersion = $cv.DisplayVersion
  releaseId = $cv.ReleaseId
} | ConvertTo-Json -Compress
"#;
    let output = Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            script,
        ])
        .output()
        .await
        .context("collect post-reboot OS evidence")?;
    if !output.status.success() {
        anyhow::bail!(
            "post-reboot OS evidence command failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    serde_json::from_slice(&output.stdout).context("parse post-reboot OS evidence")
}

#[cfg(target_os = "windows")]
fn os_matches_target(evidence: &Value, state: &ActiveUpgradeState) -> bool {
    let joined = [
        evidence
            .get("caption")
            .and_then(Value::as_str)
            .unwrap_or_default(),
        evidence
            .get("productName")
            .and_then(Value::as_str)
            .unwrap_or_default(),
        evidence
            .get("displayVersion")
            .and_then(Value::as_str)
            .unwrap_or_default(),
        evidence
            .get("releaseId")
            .and_then(Value::as_str)
            .unwrap_or_default(),
    ]
    .join(" ")
    .to_ascii_lowercase();
    let target_product = state.target_product.to_ascii_lowercase();
    let target_label = state.target_build_label.to_ascii_lowercase();
    let target_version = state.target_version.to_ascii_lowercase();

    if target_product.contains("server") || target_label.contains("server") {
        return joined.contains("server") && joined.contains(&target_version);
    }
    joined.contains("windows 11") && joined.contains(&target_version)
}

fn send_start_progress(
    outbound_tx: &mpsc::UnboundedSender<Message>,
    job: &FeatureUpgradeStartJob,
    status: &str,
    phase: &str,
    overall_percent: u8,
    error: Option<&str>,
    evidence: Value,
) -> Result<()> {
    let payload = json!({
        "operationId": &job.operation_id,
        "runId": &job.run_id,
        "organizationId": &job.organization_id,
        "agentId": &job.agent_id,
        "isoMediaId": &job.iso_media.id,
        "status": status,
        "phase": phase,
        "schemaVersion": 1,
        "eventType": "feature_upgrade.start.progress",
        "reportedAt": Utc::now().to_rfc3339(),
        "overallPercent": overall_percent,
        "phasePercent": overall_percent,
        "scheduledFor": &job.scheduled_for,
        "isoMedia": &job.iso_media,
        "setupCommandId": &job.setup_command.id,
        "error": error,
        "evidence": evidence
    });
    send_envelope(outbound_tx, "feature_upgrade_start_progress", payload)
}

fn percentage(done: u64, total: Option<u64>, min_percent: u8, max_percent: u8) -> u8 {
    let Some(total) = total.filter(|value| *value > 0) else {
        return min_percent;
    };
    let span = max_percent.saturating_sub(min_percent) as u64;
    let scaled = done.saturating_mul(span) / total;
    min_percent.saturating_add(scaled.min(span) as u8)
}

#[cfg(target_os = "windows")]
fn setup_exit_code_hex(exit_code: i32) -> String {
    format!("0x{:08X}", exit_code as u32)
}

#[cfg(target_os = "windows")]
fn setup_exit_message(exit_code: i32) -> String {
    let hex = setup_exit_code_hex(exit_code);
    let hint = match exit_code as u32 {
        0xC1900204 => Some(
            "MOSETUP_E_COMPAT_MIGCHOICE_BLOCK: Windows Setup could not use the requested /Auto Upgrade migration path",
        ),
        _ => None,
    };
    match hint {
        Some(hint) => format!("Windows Setup exited with code {hex} ({exit_code}): {hint}"),
        None => format!("Windows Setup exited with code {hex} ({exit_code})"),
    }
}

#[cfg(target_os = "windows")]
fn powershell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

#[cfg(target_os = "windows")]
fn sanitize_file_component(value: &str) -> String {
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
        "upgrade".to_string()
    } else {
        sanitized
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
    .context("serialize feature upgrade start envelope")?;
    outbound_tx
        .send(Message::Text(text))
        .map_err(|_| anyhow::anyhow!("websocket outbound channel closed"))
}

#[cfg(all(test, target_os = "windows"))]
mod tests {
    use super::*;

    #[test]
    fn expand_args_replaces_mount_and_log_tokens() {
        let args = vec![
            "/copylogs".to_string(),
            "{log_dir}".to_string(),
            "{mount_drive}\\setup.exe".to_string(),
        ];
        let expanded = expand_args(
            &args,
            "E:",
            Path::new(r"C:\Logs\Upgrade"),
            "25H2",
            Some("Professional"),
        )
        .expect("expand setup args");
        assert_eq!(
            expanded,
            vec![
                "/copylogs".to_string(),
                r"C:\Logs\Upgrade".to_string(),
                r"E:\setup.exe".to_string()
            ]
        );
    }

    #[test]
    fn expand_args_resolves_server_2025_standard_gvlk() {
        let args = vec!["/pkey".to_string(), "{target_server_gvlk}".to_string()];
        let expanded = expand_args(
            &args,
            "F:",
            Path::new(r"C:\Logs\Upgrade"),
            "2025",
            Some("ServerStandard"),
        )
        .expect("expand server key");
        assert_eq!(
            expanded,
            vec![
                "/pkey".to_string(),
                "TVRH6-WHNXV-R9WG3-9XRFY-MY832".to_string()
            ]
        );
    }

    #[test]
    fn protected_bitlocker_status_blocks_upgrade() {
        let value = json!({ "ProtectionStatus": 1 });
        assert!(bitlocker_protected_or_unknown(&value));
        let value = json!({ "ProtectionStatus": 0 });
        assert!(!bitlocker_protected_or_unknown(&value));
    }

    #[test]
    fn image_index_parser_matches_current_edition() {
        let output = r#"
Index : 1
Name : Windows 11 Home
Description : Windows 11 Home

Index : 2
Name : Windows 11 Pro
Description : Windows 11 Pro
"#;
        let current_info = WindowsEditionInfo {
            edition_id: Some("Professional".to_string()),
            product_name: Some("Windows 11 Pro".to_string()),
            installation_type: Some("Client".to_string()),
        };
        assert_eq!(
            parse_matching_image_index(output, &current_info),
            Some("2".to_string())
        );
    }

    #[test]
    fn image_index_parser_preserves_server_desktop_experience() {
        let output = r#"
Index : 1
Name : Windows Server 2025 Standard
Description : Windows Server 2025 Standard

Index : 2
Name : Windows Server 2025 Standard (Desktop Experience)
Description : Windows Server 2025 Standard (Desktop Experience)

Index : 3
Name : Windows Server 2025 Datacenter
Description : Windows Server 2025 Datacenter

Index : 4
Name : Windows Server 2025 Datacenter (Desktop Experience)
Description : Windows Server 2025 Datacenter (Desktop Experience)
"#;
        let current_info = WindowsEditionInfo {
            edition_id: Some("ServerStandard".to_string()),
            product_name: Some("Windows Server 2019 Standard".to_string()),
            installation_type: Some("Server".to_string()),
        };
        assert_eq!(
            parse_matching_image_index(output, &current_info),
            Some("2".to_string())
        );
    }

    #[test]
    fn image_index_parser_preserves_server_core() {
        let output = r#"
Index : 1
Name : Windows Server 2025 Standard
Description : Windows Server 2025 Standard

Index : 2
Name : Windows Server 2025 Standard (Desktop Experience)
Description : Windows Server 2025 Standard (Desktop Experience)
"#;
        let current_info = WindowsEditionInfo {
            edition_id: Some("ServerStandard".to_string()),
            product_name: Some("Windows Server 2019 Standard".to_string()),
            installation_type: Some("Server Core".to_string()),
        };
        assert_eq!(
            parse_matching_image_index(output, &current_info),
            Some("1".to_string())
        );
    }

    #[test]
    fn os_match_uses_client_display_version() {
        let state = ActiveUpgradeState {
            operation_id: "op".to_string(),
            run_id: "run".to_string(),
            organization_id: "org".to_string(),
            agent_id: "agent".to_string(),
            target_product: "Windows 11".to_string(),
            target_version: "25H2".to_string(),
            target_build_label: "Windows 11 25H2".to_string(),
            source_os: "Windows 11 23H2".to_string(),
            iso_media: FeatureUpgradeIsoMedia {
                id: "iso".to_string(),
                display_name: "Windows 11 25H2".to_string(),
                os_family: "windows".to_string(),
                product: "Windows 11".to_string(),
                version: "25H2".to_string(),
                edition: None,
                architecture: "x64".to_string(),
                language: Some("en-GB".to_string()),
                sha256: None,
                size_bytes: None,
                active: true,
            },
            setup_command: FeatureUpgradeSetupCommand {
                id: "matrix".to_string(),
                setup_executable: "{mount_drive}\\setup.exe".to_string(),
                arguments: vec![],
                dynamic_update_mode: Some("disable".to_string()),
                requires_eula_accept: true,
                image_index_strategy: Some("auto_match_current_edition".to_string()),
                notes: None,
            },
            log_dir: r"C:\Logs".to_string(),
            launched_at: "2026-05-26T00:00:00Z".to_string(),
            launched_boot_session_id: Some("boot-1".to_string()),
            last_verification_at: None,
        };
        assert!(os_matches_target(
            &json!({ "productName": "Windows 11 Pro", "displayVersion": "25H2" }),
            &state
        ));
    }

    #[test]
    fn windows11_language_gate_requires_exact_locale_for_22h2_and_later() {
        assert!(windows11_upgrade_requires_exact_language(
            "Windows 11",
            "25H2"
        ));
        assert!(locales_match_exactly("en-GB", "en_gb"));
        assert!(!locales_match_exactly("en-GB", "en-US"));
    }

    #[test]
    fn setup_exit_message_includes_hex_and_known_mosetup_hint() {
        let message = setup_exit_message(-1047526908);
        assert!(message.contains("0xC1900204"));
        assert!(message.contains("MOSETUP_E_COMPAT_MIGCHOICE_BLOCK"));
    }
}
