use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;
use tracing::{info, warn};
use uuid::Uuid;

use crate::collect_and_queue_full_snapshot;

const PREFLIGHT_POLL_INTERVAL: Duration = Duration::from_secs(30);
const PREFLIGHT_POLL_RESPONSE_TIMEOUT: Duration = Duration::from_secs(25);
const PREFLIGHT_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FeatureUpgradePreflightJobsEnvelope {
    pub request_id: String,
    pub jobs: Vec<FeatureUpgradePreflightJob>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
pub(crate) struct FeatureUpgradePreflightJob {
    pub operation_id: String,
    pub run_id: String,
    pub organization_id: String,
    pub agent_id: String,
    pub source_os: String,
    pub target_product: String,
    pub target_version: String,
    pub target_build_label: String,
    #[serde(default)]
    pub snapshot_request_id: Option<String>,
    #[serde(default)]
    pub checks: Vec<PreflightCheckDefinition>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PreflightCheckDefinition {
    pub id: String,
    pub label: String,
    pub severity: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub requires_fresh_snapshot: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FeatureUpgradePreflightJobsAvailablePayload {
    pub reason: Option<String>,
    pub requested_by: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FeatureUpgradePreflightJobsPollPayload {
    request_id: String,
    limit: u32,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FeatureUpgradePreflightProgressPayload {
    operation_id: String,
    run_id: String,
    organization_id: String,
    agent_id: String,
    status: String,
    phase: String,
    checks: Vec<Value>,
}

pub(crate) fn start_preflight_manager(
    agent_id: String,
    hostname: String,
    boot_session_id: String,
    outbound_tx: mpsc::UnboundedSender<Message>,
    mut jobs_rx: mpsc::UnboundedReceiver<FeatureUpgradePreflightJobsEnvelope>,
    mut wake_rx: mpsc::UnboundedReceiver<()>,
    snapshot_in_progress: Arc<AtomicBool>,
) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(PREFLIGHT_POLL_INTERVAL);
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

            let jobs = match poll_preflight_jobs(&outbound_tx, &mut jobs_rx).await {
                Ok(jobs) => jobs,
                Err(error) => {
                    warn!(%error, "feature upgrade preflight job poll failed");
                    continue;
                }
            };

            for job in jobs {
                if let Err(error) = run_preflight_job(
                    &outbound_tx,
                    &agent_id,
                    &hostname,
                    &boot_session_id,
                    &snapshot_in_progress,
                    job,
                )
                .await
                {
                    warn!(%error, "feature upgrade preflight snapshot request failed before status could be reported");
                }
            }
        }
    });
}

pub(crate) fn send_preflight_jobs_available_signal(
    wake_tx: &mpsc::UnboundedSender<()>,
    payload: FeatureUpgradePreflightJobsAvailablePayload,
) {
    info!(
        reason = ?payload.reason,
        requested_by = ?payload.requested_by,
        "feature upgrade preflight jobs available; waking preflight manager"
    );
    let _ = wake_tx.send(());
}

async fn poll_preflight_jobs(
    outbound_tx: &mpsc::UnboundedSender<Message>,
    jobs_rx: &mut mpsc::UnboundedReceiver<FeatureUpgradePreflightJobsEnvelope>,
) -> Result<Vec<FeatureUpgradePreflightJob>> {
    let request_id = Uuid::new_v4().to_string();
    send_envelope(
        outbound_tx,
        "feature_upgrade_preflight_jobs_poll",
        FeatureUpgradePreflightJobsPollPayload {
            request_id: request_id.clone(),
            limit: 1,
        },
    )?;

    tokio::time::timeout(PREFLIGHT_POLL_RESPONSE_TIMEOUT, async {
        while let Some(payload) = jobs_rx.recv().await {
            if payload.request_id == request_id {
                return Ok(payload.jobs);
            }
            warn!(
                expected_request_id = %request_id,
                received_request_id = %payload.request_id,
                "discarding stale feature upgrade preflight jobs response"
            );
        }
        Err(anyhow::anyhow!(
            "feature upgrade preflight jobs response channel closed"
        ))
    })
    .await
    .context("feature upgrade preflight jobs poll timed out")?
}

async fn run_preflight_job(
    outbound_tx: &mpsc::UnboundedSender<Message>,
    agent_id: &str,
    hostname: &str,
    boot_session_id: &str,
    snapshot_in_progress: &Arc<AtomicBool>,
    job: FeatureUpgradePreflightJob,
) -> Result<()> {
    send_preflight_progress(outbound_tx, &job, "running", "checking", Vec::new())?;

    let heartbeat_stop = Arc::new(AtomicBool::new(false));
    let heartbeat_stop_for_task = heartbeat_stop.clone();
    let heartbeat_tx = outbound_tx.clone();
    let heartbeat_job = job.clone();
    let heartbeat = tokio::spawn(async move {
        loop {
            tokio::time::sleep(PREFLIGHT_HEARTBEAT_INTERVAL).await;
            if heartbeat_stop_for_task.load(Ordering::SeqCst) {
                break;
            }
            let _ = send_preflight_progress(
                &heartbeat_tx,
                &heartbeat_job,
                "running",
                "checking",
                Vec::new(),
            );
        }
    });

    let snapshot_request_id = snapshot_request_id_for_job(&job).to_string();
    let snapshot_result = collect_and_queue_full_snapshot(
        outbound_tx,
        agent_id,
        hostname,
        boot_session_id,
        Some(snapshot_request_id.clone()),
        snapshot_in_progress,
        "feature_upgrade_preflight",
    )
    .await;

    heartbeat_stop.store(true, Ordering::SeqCst);
    heartbeat.abort();

    match snapshot_result {
        Ok(pending_update_count) => {
            info!(
                operation_id = %job.operation_id,
                snapshot_request_id = %snapshot_request_id,
                pending_update_count,
                "feature upgrade preflight full_snapshot queued"
            );
            send_preflight_progress(outbound_tx, &job, "running", "checking", Vec::new())?;
        }
        Err(error) => {
            let checks = snapshot_failure_checks(&job, &format!("{error:#}"));
            send_preflight_progress(outbound_tx, &job, "failed", "failed", checks)?;
            return Err(error);
        }
    }

    Ok(())
}

fn snapshot_request_id_for_job(job: &FeatureUpgradePreflightJob) -> &str {
    job.snapshot_request_id
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(job.operation_id.as_str())
}

fn snapshot_failure_checks(job: &FeatureUpgradePreflightJob, error: &str) -> Vec<Value> {
    job.checks
        .iter()
        .map(|check| {
            let status = if check.requires_fresh_snapshot && check.severity == "required" {
                "failed"
            } else if check.requires_fresh_snapshot {
                "warning"
            } else {
                "skipped"
            };
            let message = if check.requires_fresh_snapshot {
                "Full snapshot collection failed before this evidence could be refreshed"
            } else {
                "Skipped because preflight snapshot collection failed"
            };
            json!({
                "id": check.id,
                "label": check.label,
                "severity": check.severity,
                "status": status,
                "message": message,
                "description": check.description,
                "requiresFreshSnapshot": check.requires_fresh_snapshot,
                "source": "snapshot",
                "sourceLabel": "Fresh preflight snapshot",
                "sourceUpdatedAt": null,
                "details": { "error": error }
            })
        })
        .collect()
}

fn send_preflight_progress(
    outbound_tx: &mpsc::UnboundedSender<Message>,
    job: &FeatureUpgradePreflightJob,
    status: &str,
    phase: &str,
    checks: Vec<Value>,
) -> Result<()> {
    send_envelope(
        outbound_tx,
        "feature_upgrade_preflight_progress",
        FeatureUpgradePreflightProgressPayload {
            operation_id: job.operation_id.clone(),
            run_id: job.run_id.clone(),
            organization_id: job.organization_id.clone(),
            agent_id: job.agent_id.clone(),
            status: status.to_string(),
            phase: phase.to_string(),
            checks,
        },
    )
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
    .context("serialize feature upgrade preflight envelope")?;
    outbound_tx
        .send(Message::Text(text))
        .map_err(|_| anyhow::anyhow!("websocket outbound channel closed"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job_with_snapshot(snapshot_request_id: Option<String>) -> FeatureUpgradePreflightJob {
        FeatureUpgradePreflightJob {
            operation_id: "operation-1".to_string(),
            run_id: "run-1".to_string(),
            organization_id: "org-1".to_string(),
            agent_id: "agent-1".to_string(),
            source_os: "Windows 11 Pro 23H2".to_string(),
            target_product: "Windows 11".to_string(),
            target_version: "25H2".to_string(),
            target_build_label: "Windows 11 25H2".to_string(),
            snapshot_request_id,
            checks: vec![],
        }
    }

    #[test]
    fn snapshot_request_id_defaults_to_operation_id() {
        let job = job_with_snapshot(None);
        assert_eq!(snapshot_request_id_for_job(&job), "operation-1");
    }

    #[test]
    fn snapshot_request_id_uses_payload_value() {
        let job = job_with_snapshot(Some("snapshot-1".to_string()));
        assert_eq!(snapshot_request_id_for_job(&job), "snapshot-1");
    }

    #[test]
    fn snapshot_failure_blocks_required_fresh_checks_only() {
        let mut job = job_with_snapshot(None);
        job.checks = vec![
            PreflightCheckDefinition {
                id: "disk_space".to_string(),
                label: "Disk space".to_string(),
                severity: "required".to_string(),
                description: None,
                requires_fresh_snapshot: true,
            },
            PreflightCheckDefinition {
                id: "bitlocker".to_string(),
                label: "BitLocker".to_string(),
                severity: "warning".to_string(),
                description: None,
                requires_fresh_snapshot: true,
            },
            PreflightCheckDefinition {
                id: "pending_reboot".to_string(),
                label: "Pending reboot".to_string(),
                severity: "required".to_string(),
                description: None,
                requires_fresh_snapshot: false,
            },
        ];

        let checks = snapshot_failure_checks(&job, "boom");
        assert_eq!(checks[0]["status"], "failed");
        assert_eq!(checks[1]["status"], "warning");
        assert_eq!(checks[2]["status"], "skipped");
    }
}
