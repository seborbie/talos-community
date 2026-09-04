use std::{
    collections::HashSet,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};

#[cfg(target_os = "macos")]
use std::{
    fs::{File, OpenOptions},
    io::{Read, Write},
    os::unix::{ffi::OsStrExt, fs::OpenOptionsExt},
    path::{Path, PathBuf},
    process::Stdio,
    sync::{Mutex, OnceLock},
    time::{Instant, SystemTime, UNIX_EPOCH},
};

#[cfg(target_os = "linux")]
use std::{
    collections::HashMap,
    fs::File,
    io::Read,
    process::Stdio,
    time::{Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;
use tracing::{info, warn};
use uuid::Uuid;

use crate::{collect_full_snapshot_update, OutgoingEnvelope};
#[cfg(target_os = "macos")]
use talos_protocol::MacosUpdateAccountStatusPayload;
#[cfg(any(target_os = "windows", target_os = "macos"))]
use talos_protocol::{
    RebootNoticeAction, WorkerChatControlPayload, CHAT_MSG_AUTH, CHAT_MSG_CONTROL,
};

const PATCH_POLL_INTERVAL: Duration = Duration::from_secs(30);
const PATCH_POLL_RESPONSE_TIMEOUT: Duration = Duration::from_secs(25);
const PATCH_STATE_CHECKIN_INTERVAL: Duration = Duration::from_secs(60 * 60);
const DEFAULT_PATCH_PLAN_RESPONSE_TIMEOUT: Duration = Duration::from_secs(120);
const PATCH_INSTALL_INTENT_ID: &str = "talos.patch.install";
#[cfg(target_os = "windows")]
const WU_UPGRADES_CATEGORY_ID: &str = "3689BDC8-B205-4AF4-8D4A-A63924C5E9D5";
const PATCH_SCAN_PROGRESS_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(30);
const PATCH_JOB_PROGRESS_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(30);
const DEFAULT_PATCH_COMMAND_TIMEOUT_SECS: u64 = 7200;
const MIN_PATCH_COMMAND_TIMEOUT_SECS: u64 = 60;
const MAX_PATCH_COMMAND_TIMEOUT_SECS: u64 = 86_400;
const PATCH_REBOOT_MESSAGE: &str =
    "Talos patch management installed updates and requires a restart.";
#[cfg(any(target_os = "windows", target_os = "macos"))]
const UPDATE_REBOOT_NOTICE_WARNING: Duration = Duration::from_secs(15 * 60);
#[cfg(any(target_os = "windows", target_os = "macos"))]
const UPDATE_REBOOT_NOTICE_DELAY: Duration = Duration::from_secs(15 * 60);
#[cfg(any(target_os = "windows", target_os = "macos"))]
const UPDATE_REBOOT_NOTICE_MAX_DEFERRALS: u32 = 4;
#[cfg(any(target_os = "windows", target_os = "macos"))]
const UPDATE_REBOOT_NOTICE_CONNECT_TIMEOUT: Duration = Duration::from_secs(45);

fn patch_command_timeout_from_env_value(value: Option<&str>) -> Duration {
    let seconds = value
        .and_then(|raw| raw.trim().parse::<u64>().ok())
        .unwrap_or(DEFAULT_PATCH_COMMAND_TIMEOUT_SECS)
        .clamp(
            MIN_PATCH_COMMAND_TIMEOUT_SECS,
            MAX_PATCH_COMMAND_TIMEOUT_SECS,
        );
    Duration::from_secs(seconds)
}

#[cfg(target_os = "linux")]
fn linux_patch_command_timeout() -> Duration {
    let value = std::env::var("RMM_PATCH_COMMAND_TIMEOUT_SECS").ok();
    patch_command_timeout_from_env_value(value.as_deref())
}

#[derive(Debug, Clone, Copy)]
enum SnapshotRequestStatus {
    Started,
    Coalesced,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PatchJobStatusMode {
    LegacyRemediationJob,
    ActionPlanOperation,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PatchJobsEnvelope {
    pub request_id: String,
    pub jobs: Vec<PatchRemediationJob>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PatchActionPlanEnvelope {
    pub request_id: Option<String>,
    pub plan: PatchActionPlan,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
pub(crate) struct PatchActionPlan {
    pub schema_version: u32,
    pub generated_at: String,
    pub organization_id: Option<String>,
    pub agent_id: String,
    pub policy_id: Option<String>,
    pub managed_mode: bool,
    pub native_windows_update_control: bool,
    pub next_check_in_at: Option<String>,
    #[serde(default)]
    pub actions: Vec<PatchActionPlanItem>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PatchActionPlanItem {
    pub operation_id: String,
    pub action: String,
    #[serde(default)]
    pub update_keys: Vec<String>,
    pub window: Option<String>,
    pub not_before: Option<String>,
    pub deadline_at: Option<String>,
    pub forced: bool,
    pub reason: String,
    #[serde(default)]
    pub metadata: Value,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PatchRemediationJob {
    pub id: String,
    pub organization_id: String,
    pub agent_id: String,
    pub intent_id: String,
    pub status: String,
    pub dedupe_key: Option<String>,
    pub metadata: Value,
    pub requested_at: String,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    #[serde(default)]
    pub steps: Vec<PatchRemediationStep>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PatchRemediationStep {
    pub id: String,
    pub step_index: i32,
    pub command: String,
    pub status: String,
    pub evidence: Option<Value>,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PatchJobsAvailablePayload {
    pub reason: Option<String>,
    pub requested_by: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PatchJobsPollPayload {
    request_id: String,
    limit: u32,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PatchJobUpdatePayload {
    job_id: String,
    status: String,
    step_index: i32,
    evidence: Value,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PatchStateCheckinPayload {
    request_id: String,
    observed_at: String,
    state: Value,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PatchActionResultPayload {
    operation_id: String,
    action: String,
    status: String,
    update_keys: Vec<String>,
    evidence: Value,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct PatchUpdateIdentity {
    update_key: String,
    title_norm: String,
    kb_norm: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AptPatchCandidate {
    package: String,
    source: Option<String>,
    target_version: String,
    current_version: Option<String>,
    architecture: Option<String>,
    title: String,
    description: String,
    identity: PatchUpdateIdentity,
    requires_reboot: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RpmPatchCandidate {
    package: String,
    source: Option<String>,
    target_version: String,
    current_version: Option<String>,
    architecture: Option<String>,
    title: String,
    description: String,
    identity: PatchUpdateIdentity,
    requires_reboot: bool,
}

#[cfg(any(target_os = "macos", test))]
#[derive(Debug, Clone, PartialEq, Eq)]
struct MacosPatchCandidate {
    label: String,
    title: String,
    version: Option<String>,
    size: Option<String>,
    recommended: bool,
    requires_reboot: bool,
    identity: PatchUpdateIdentity,
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LinuxPatchPackageManager {
    Apt,
    Dnf,
    Yum,
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
impl LinuxPatchPackageManager {
    fn command(self) -> &'static str {
        match self {
            LinuxPatchPackageManager::Apt => "apt-get",
            LinuxPatchPackageManager::Dnf => "dnf",
            LinuxPatchPackageManager::Yum => "yum",
        }
    }

    fn label(self) -> &'static str {
        match self {
            LinuxPatchPackageManager::Apt => "apt",
            LinuxPatchPackageManager::Dnf => "dnf",
            LinuxPatchPackageManager::Yum => "yum",
        }
    }
}

#[derive(Debug)]
pub(crate) struct PatchExecutionOutcome {
    pub(crate) status: &'static str,
    pub(crate) evidence: Value,
    pub(crate) force_reboot_after_report: bool,
}

#[derive(Clone)]
pub(crate) struct PatchProgressReporter {
    outbound_tx: mpsc::UnboundedSender<Message>,
    job_id: String,
    command_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PatchProgressSnapshot {
    phase: &'static str,
    overall_percent: i32,
    phase_percent: i32,
    current_update_index: Option<i32>,
    current_update_percent: Option<i32>,
}

pub(crate) fn start_patch_manager(
    agent_id: String,
    hostname: String,
    boot_session_id: String,
    outbound_tx: mpsc::UnboundedSender<Message>,
    mut jobs_rx: mpsc::UnboundedReceiver<PatchJobsEnvelope>,
    mut plans_rx: mpsc::UnboundedReceiver<PatchActionPlanEnvelope>,
    mut wake_rx: mpsc::UnboundedReceiver<()>,
    snapshot_in_progress: Arc<AtomicBool>,
    pending_patch_snapshot: Arc<AtomicBool>,
) {
    tokio::spawn(async move {
        let mut job_interval = tokio::time::interval(PATCH_POLL_INTERVAL);
        let mut checkin_interval = tokio::time::interval(PATCH_STATE_CHECKIN_INTERVAL);
        job_interval.tick().await;
        checkin_interval.tick().await;

        if let Err(error) = request_and_execute_patch_plan(
            &outbound_tx,
            &mut plans_rx,
            &agent_id,
            &hostname,
            &boot_session_id,
            &snapshot_in_progress,
            &pending_patch_snapshot,
        )
        .await
        {
            warn!(%error, "initial patch state check-in failed");
        }

        loop {
            enum Work {
                PollJobs,
                CheckIn,
                Wake,
            }
            let work = tokio::select! {
                _ = job_interval.tick() => Work::PollJobs,
                _ = checkin_interval.tick() => Work::CheckIn,
                wake = wake_rx.recv() => {
                    if wake.is_none() {
                        break;
                    }
                    Work::Wake
                }
            };

            if matches!(work, Work::CheckIn | Work::Wake) {
                if let Err(error) = request_and_execute_patch_plan(
                    &outbound_tx,
                    &mut plans_rx,
                    &agent_id,
                    &hostname,
                    &boot_session_id,
                    &snapshot_in_progress,
                    &pending_patch_snapshot,
                )
                .await
                {
                    warn!(%error, "patch state check-in failed");
                }
            }

            if !matches!(work, Work::PollJobs | Work::Wake) {
                continue;
            }

            let jobs = match poll_patch_jobs(&outbound_tx, &mut jobs_rx).await {
                Ok(jobs) => jobs,
                Err(error) => {
                    warn!(%error, "patch job poll failed");
                    continue;
                }
            };

            for job in jobs {
                if job.intent_id != PATCH_INSTALL_INTENT_ID {
                    warn!(
                        job_id = %job.id,
                        intent_id = %job.intent_id,
                        "ignoring non-patch remediation job returned to patch manager"
                    );
                    continue;
                }

                if let Err(error) = run_patch_job(
                    &outbound_tx,
                    job,
                    PatchJobStatusMode::LegacyRemediationJob,
                    &agent_id,
                    &hostname,
                    &boot_session_id,
                    &snapshot_in_progress,
                    &pending_patch_snapshot,
                )
                .await
                {
                    warn!(%error, "patch job execution failed before status could be reported");
                }
            }
        }
    });
}

pub(crate) fn send_patch_jobs_available_signal(
    wake_tx: &mpsc::UnboundedSender<()>,
    payload: PatchJobsAvailablePayload,
) {
    info!(
        reason = ?payload.reason,
        requested_by = ?payload.requested_by,
        "patch jobs available; waking patch manager"
    );
    let _ = wake_tx.send(());
}

pub(crate) fn progress_reporter(
    outbound_tx: &mpsc::UnboundedSender<Message>,
    job: &PatchRemediationJob,
) -> PatchProgressReporter {
    PatchProgressReporter {
        outbound_tx: outbound_tx.clone(),
        job_id: job.id.clone(),
        command_id: job.id.clone(),
    }
}

async fn poll_patch_jobs(
    outbound_tx: &mpsc::UnboundedSender<Message>,
    jobs_rx: &mut mpsc::UnboundedReceiver<PatchJobsEnvelope>,
) -> Result<Vec<PatchRemediationJob>> {
    let request_id = Uuid::new_v4().to_string();
    send_envelope(
        outbound_tx,
        "patch_jobs_poll",
        PatchJobsPollPayload {
            request_id: request_id.clone(),
            limit: 1,
        },
    )?;

    let response = tokio::time::timeout(PATCH_POLL_RESPONSE_TIMEOUT, async {
        while let Some(payload) = jobs_rx.recv().await {
            if payload.request_id == request_id {
                return Ok(payload.jobs);
            }
            warn!(
                expected_request_id = %request_id,
                received_request_id = %payload.request_id,
                "discarding stale patch jobs response"
            );
        }
        Err(anyhow::anyhow!("patch jobs response channel closed"))
    })
    .await
    .context("patch jobs poll timed out")??;

    Ok(response)
}

async fn request_and_execute_patch_plan(
    outbound_tx: &mpsc::UnboundedSender<Message>,
    plans_rx: &mut mpsc::UnboundedReceiver<PatchActionPlanEnvelope>,
    agent_id: &str,
    hostname: &str,
    boot_session_id: &str,
    snapshot_in_progress: &Arc<AtomicBool>,
    pending_patch_snapshot: &Arc<AtomicBool>,
) -> Result<()> {
    let plan = request_patch_action_plan(outbound_tx, plans_rx, agent_id).await?;
    execute_patch_action_plan(
        outbound_tx,
        plan,
        agent_id,
        hostname,
        boot_session_id,
        snapshot_in_progress,
        pending_patch_snapshot,
    )
    .await
}

async fn request_patch_action_plan(
    outbound_tx: &mpsc::UnboundedSender<Message>,
    plans_rx: &mut mpsc::UnboundedReceiver<PatchActionPlanEnvelope>,
    agent_id: &str,
) -> Result<PatchActionPlan> {
    let request_id = Uuid::new_v4().to_string();
    send_envelope(
        outbound_tx,
        "patch_state_checkin",
        PatchStateCheckinPayload {
            request_id: request_id.clone(),
            observed_at: Utc::now().to_rfc3339(),
            state: collect_patch_executor_state(agent_id),
        },
    )?;

    let response = tokio::time::timeout(patch_plan_response_timeout(), async {
        while let Some(payload) = plans_rx.recv().await {
            if payload.request_id.as_deref() == Some(request_id.as_str()) {
                return Ok(payload.plan);
            }
            warn!(
                expected_request_id = %request_id,
                received_request_id = ?payload.request_id,
                "discarding stale patch action plan"
            );
        }
        Err(anyhow::anyhow!("patch action plan channel closed"))
    })
    .await
    .context("patch action plan request timed out")??;

    Ok(response)
}

fn patch_plan_response_timeout() -> Duration {
    std::env::var("RMM_PATCH_PLAN_RESPONSE_TIMEOUT_SECS")
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .filter(|seconds| *seconds > 0)
        .map(Duration::from_secs)
        .unwrap_or(DEFAULT_PATCH_PLAN_RESPONSE_TIMEOUT)
}

fn collect_patch_executor_state(agent_id: &str) -> Value {
    json!({
        "agentId": agent_id,
        "reportedAt": Utc::now().to_rfc3339(),
        "nativeWindowsUpdateControlApplied": current_native_windows_update_control_state().unwrap_or(Value::Null)
    })
}

async fn execute_patch_action_plan(
    outbound_tx: &mpsc::UnboundedSender<Message>,
    plan: PatchActionPlan,
    agent_id: &str,
    hostname: &str,
    boot_session_id: &str,
    snapshot_in_progress: &Arc<AtomicBool>,
    pending_patch_snapshot: &Arc<AtomicBool>,
) -> Result<()> {
    if plan.agent_id != agent_id {
        warn!(
            connected_agent_id = %agent_id,
            plan_agent_id = %plan.agent_id,
            "ignoring patch plan for a different agent"
        );
        return Ok(());
    }

    for item in plan.actions.clone() {
        match item.action.as_str() {
            "applyNativeControl" => {
                let enabled = item
                    .metadata
                    .get("enabled")
                    .and_then(Value::as_bool)
                    .unwrap_or(plan.native_windows_update_control);
                let result = apply_native_windows_update_control(enabled).await;
                report_patch_action_result(
                    outbound_tx,
                    &item,
                    if result.is_ok() {
                        "completed"
                    } else {
                        "failed"
                    },
                    json!({
                        "enabled": enabled,
                        "error": result.err().map(|error| format!("{error:#}"))
                    }),
                )?;
            }
            "scan" => {
                let (status, evidence) = run_patch_scan_action(
                    outbound_tx,
                    &plan,
                    &item,
                    agent_id,
                    hostname,
                    boot_session_id,
                    snapshot_in_progress,
                    pending_patch_snapshot,
                )
                .await;
                report_patch_action_result(outbound_tx, &item, status, evidence)?;
            }
            "download" => {
                let mut job = action_item_to_patch_job(&plan, &item, "download");
                job.metadata["sourceAction"] = json!("download");
                job.metadata["rebootBehavior"] = json!("suppress");
                let outcome = match run_patch_job(
                    outbound_tx,
                    job,
                    PatchJobStatusMode::ActionPlanOperation,
                    agent_id,
                    hostname,
                    boot_session_id,
                    snapshot_in_progress,
                    pending_patch_snapshot,
                )
                .await
                {
                    Ok(outcome) => outcome,
                    Err(error) => {
                        report_patch_action_result(
                            outbound_tx,
                            &item,
                            "failed",
                            json!({ "error": format!("{error:#}") }),
                        )?;
                        continue;
                    }
                };
                report_patch_action_result(outbound_tx, &item, outcome.status, outcome.evidence)?;
            }
            "install" => {
                let job = action_item_to_patch_job(&plan, &item, "install");
                let outcome = match run_patch_job(
                    outbound_tx,
                    job,
                    PatchJobStatusMode::ActionPlanOperation,
                    agent_id,
                    hostname,
                    boot_session_id,
                    snapshot_in_progress,
                    pending_patch_snapshot,
                )
                .await
                {
                    Ok(outcome) => outcome,
                    Err(error) => {
                        report_patch_action_result(
                            outbound_tx,
                            &item,
                            "failed",
                            json!({ "error": format!("{error:#}") }),
                        )?;
                        continue;
                    }
                };
                report_patch_action_result(outbound_tx, &item, outcome.status, outcome.evidence)?;
            }
            "reboot" => {
                let result = tokio::task::spawn_blocking(schedule_forced_reboot_after_patch).await;
                let status = match &result {
                    Ok(Ok(_)) => "completed",
                    _ => "failed",
                };
                report_patch_action_result(
                    outbound_tx,
                    &item,
                    status,
                    json!({
                        "forced": item.forced,
                        "rebootScheduled": matches!(&result, Ok(Ok(_))),
                        "schedule": match &result {
                            Ok(Ok(evidence)) => evidence.clone(),
                            _ => Value::Null
                        },
                        "error": match result {
                            Ok(Ok(_)) => Value::Null,
                            Ok(Err(error)) => json!(format!("{error:#}")),
                            Err(error) => json!(format!("{error:#}"))
                        }
                    }),
                )?;
            }
            "defer" | "blocked" | "reportOnly" => {
                report_patch_action_result(
                    outbound_tx,
                    &item,
                    "acknowledged",
                    json!({
                        "reason": item.reason,
                        "window": item.window,
                        "notBefore": item.not_before,
                        "deadlineAt": item.deadline_at
                    }),
                )?;
            }
            other => {
                warn!(action = other, "ignoring unknown patch action");
            }
        }
    }

    Ok(())
}

fn action_item_to_patch_job(
    plan: &PatchActionPlan,
    item: &PatchActionPlanItem,
    mode: &'static str,
) -> PatchRemediationJob {
    let reboot_behavior = item
        .metadata
        .get("rebootBehavior")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("allow")
        .to_string();

    PatchRemediationJob {
        id: item.operation_id.clone(),
        organization_id: plan.organization_id.clone().unwrap_or_default(),
        agent_id: plan.agent_id.clone(),
        intent_id: PATCH_INSTALL_INTENT_ID.to_string(),
        status: "queued".to_string(),
        dedupe_key: Some(format!("patch-plan:{}", item.operation_id)),
        metadata: json!({
            "source": "patch_action_plan",
            "policyId": plan.policy_id,
            "operationId": item.operation_id,
            "mode": mode,
            "downloadOnly": mode == "download",
            "updateKeys": item.update_keys,
            "rebootBehavior": reboot_behavior,
            "reason": item.reason
        }),
        requested_at: plan.generated_at.clone(),
        started_at: None,
        finished_at: None,
        steps: Vec::new(),
    }
}

fn report_patch_action_result(
    outbound_tx: &mpsc::UnboundedSender<Message>,
    item: &PatchActionPlanItem,
    status: &str,
    evidence: Value,
) -> Result<()> {
    send_envelope(
        outbound_tx,
        "patch_action_result",
        PatchActionResultPayload {
            operation_id: item.operation_id.clone(),
            action: item.action.clone(),
            status: status.to_string(),
            update_keys: item.update_keys.clone(),
            evidence,
        },
    )
}

async fn run_patch_scan_action(
    outbound_tx: &mpsc::UnboundedSender<Message>,
    plan: &PatchActionPlan,
    item: &PatchActionPlanItem,
    agent_id: &str,
    hostname: &str,
    boot_session_id: &str,
    snapshot_in_progress: &Arc<AtomicBool>,
    pending_patch_snapshot: &Arc<AtomicBool>,
) -> (&'static str, Value) {
    let organization_id = plan.organization_id.clone().unwrap_or_default();
    let operation_id = item.operation_id.clone();
    let reason = item.reason.clone();

    send_patch_scan_progress(
        outbound_tx,
        &organization_id,
        agent_id,
        &operation_id,
        "running",
        None,
        None,
    );

    let heartbeat_stop = Arc::new(AtomicBool::new(false));
    let heartbeat_stop_for_task = heartbeat_stop.clone();
    let heartbeat_tx = outbound_tx.clone();
    let heartbeat_organization_id = organization_id.clone();
    let heartbeat_agent_id = agent_id.to_string();
    let heartbeat_operation_id = operation_id.clone();
    let heartbeat_handle = tokio::spawn(async move {
        loop {
            tokio::time::sleep(PATCH_SCAN_PROGRESS_HEARTBEAT_INTERVAL).await;
            if heartbeat_stop_for_task.load(Ordering::SeqCst) {
                break;
            }
            send_patch_scan_progress(
                &heartbeat_tx,
                &heartbeat_organization_id,
                &heartbeat_agent_id,
                &heartbeat_operation_id,
                "running",
                None,
                None,
            );
        }
    });

    let mut coalesced = false;
    while snapshot_in_progress
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        coalesced = true;
        tokio::time::sleep(Duration::from_secs(2)).await;
    }

    let snapshot_result = match refresh_patch_metadata_before_scan().await {
        Ok(()) => {
            collect_and_send_post_patch_snapshot(outbound_tx, agent_id, hostname, boot_session_id)
                .await
        }
        Err(error) => Err(error),
    };
    snapshot_in_progress.store(false, Ordering::SeqCst);
    if pending_patch_snapshot.load(Ordering::SeqCst) {
        spawn_pending_patch_snapshot_watcher(
            outbound_tx.clone(),
            agent_id.to_string(),
            hostname.to_string(),
            boot_session_id.to_string(),
            snapshot_in_progress.clone(),
            pending_patch_snapshot.clone(),
        );
    }

    heartbeat_stop.store(true, Ordering::SeqCst);
    heartbeat_handle.abort();

    match snapshot_result {
        Ok(pending_update_count) => {
            send_patch_scan_progress(
                outbound_tx,
                &organization_id,
                agent_id,
                &operation_id,
                "completed",
                Some(pending_update_count),
                None,
            );
            (
                "completed",
                json!({
                    "snapshotRequested": true,
                    "snapshotQueued": true,
                    "coalesced": coalesced,
                    "pendingUpdateCount": pending_update_count,
                    "reason": reason
                }),
            )
        }
        Err(error) => {
            let message = format!("{error:#}");
            send_patch_scan_progress(
                outbound_tx,
                &organization_id,
                agent_id,
                &operation_id,
                "failed",
                None,
                Some(message.clone()),
            );
            (
                "failed",
                json!({
                    "snapshotRequested": true,
                    "snapshotQueued": true,
                    "coalesced": coalesced,
                    "reason": reason,
                    "error": message
                }),
            )
        }
    }
}

async fn run_patch_job(
    outbound_tx: &mpsc::UnboundedSender<Message>,
    job: PatchRemediationJob,
    status_mode: PatchJobStatusMode,
    agent_id: &str,
    hostname: &str,
    boot_session_id: &str,
    snapshot_in_progress: &Arc<AtomicBool>,
    pending_patch_snapshot: &Arc<AtomicBool>,
) -> Result<PatchExecutionOutcome> {
    let report_legacy_status = matches!(status_mode, PatchJobStatusMode::LegacyRemediationJob);
    if report_legacy_status {
        send_patch_job_update(
            outbound_tx,
            &job.id,
            "running",
            json!({
                "phase": "running",
                "job": job_summary(&job),
                "updates": [],
                "summary": empty_summary(),
                "error": null
            }),
        )?;
    }

    let reporter = progress_reporter(outbound_tx, &job);
    let job_for_execution = job.clone();
    let mut outcome = tokio::task::spawn_blocking(move || {
        execute_patch_job_blocking_with_progress(job_for_execution, Some(reporter))
    })
    .await
    .context("patch execution task failed")?;

    if report_legacy_status {
        send_patch_job_update(
            outbound_tx,
            &job.id,
            outcome.status,
            outcome.evidence.clone(),
        )?;
    }
    request_snapshot_after_patch(
        outbound_tx,
        agent_id,
        hostname,
        boot_session_id,
        snapshot_in_progress,
        pending_patch_snapshot,
    );

    if outcome.force_reboot_after_report {
        match tokio::task::spawn_blocking(schedule_forced_reboot_after_patch).await {
            Ok(Ok(schedule_evidence)) => {
                outcome.evidence["rebootScheduled"] = json!(true);
                outcome.evidence["rebootSchedule"] = schedule_evidence;
                if report_legacy_status {
                    let _ = send_patch_job_update(
                        outbound_tx,
                        &job.id,
                        outcome.status,
                        outcome.evidence.clone(),
                    );
                }
            }
            Ok(Err(error)) => {
                warn!(%error, "failed to schedule forced reboot after patch install");
            }
            Err(error) => {
                warn!(%error, "forced reboot scheduling task failed");
            }
        }
    }

    Ok(outcome)
}

fn request_snapshot_after_patch(
    outbound_tx: &mpsc::UnboundedSender<Message>,
    agent_id: &str,
    hostname: &str,
    boot_session_id: &str,
    snapshot_in_progress: &Arc<AtomicBool>,
    pending_patch_snapshot: &Arc<AtomicBool>,
) -> SnapshotRequestStatus {
    if snapshot_in_progress
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        let already_pending = pending_patch_snapshot.swap(true, Ordering::SeqCst);
        info!(
            already_pending,
            "post-patch snapshot coalesced because another snapshot is in progress"
        );
        if !already_pending {
            spawn_pending_patch_snapshot_watcher(
                outbound_tx.clone(),
                agent_id.to_string(),
                hostname.to_string(),
                boot_session_id.to_string(),
                snapshot_in_progress.clone(),
                pending_patch_snapshot.clone(),
            );
        }
        return SnapshotRequestStatus::Coalesced;
    }

    spawn_post_patch_snapshot(
        outbound_tx.clone(),
        agent_id.to_string(),
        hostname.to_string(),
        boot_session_id.to_string(),
        snapshot_in_progress.clone(),
        pending_patch_snapshot.clone(),
    );
    SnapshotRequestStatus::Started
}

fn spawn_pending_patch_snapshot_watcher(
    outbound_tx: mpsc::UnboundedSender<Message>,
    agent_id: String,
    hostname: String,
    boot_session_id: String,
    snapshot_in_progress: Arc<AtomicBool>,
    pending_patch_snapshot: Arc<AtomicBool>,
) {
    tokio::spawn(async move {
        loop {
            if !pending_patch_snapshot.load(Ordering::SeqCst) {
                return;
            }

            if snapshot_in_progress
                .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
            {
                pending_patch_snapshot.store(false, Ordering::SeqCst);
                if let Err(error) = collect_and_send_post_patch_snapshot(
                    &outbound_tx,
                    &agent_id,
                    &hostname,
                    &boot_session_id,
                )
                .await
                {
                    warn!(%error, "pending post-patch full snapshot failed");
                }
                snapshot_in_progress.store(false, Ordering::SeqCst);
                continue;
            }

            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    });
}

fn spawn_post_patch_snapshot(
    outbound_tx: mpsc::UnboundedSender<Message>,
    agent_id: String,
    hostname: String,
    boot_session_id: String,
    snapshot_in_progress: Arc<AtomicBool>,
    pending_patch_snapshot: Arc<AtomicBool>,
) {
    tokio::spawn(async move {
        if let Err(error) = collect_and_send_post_patch_snapshot(
            &outbound_tx,
            &agent_id,
            &hostname,
            &boot_session_id,
        )
        .await
        {
            warn!(%error, "post-patch full snapshot failed");
        }
        snapshot_in_progress.store(false, Ordering::SeqCst);
        if pending_patch_snapshot.load(Ordering::SeqCst) {
            spawn_pending_patch_snapshot_watcher(
                outbound_tx,
                agent_id,
                hostname,
                boot_session_id,
                snapshot_in_progress,
                pending_patch_snapshot,
            );
        }
    });
}

async fn collect_and_send_post_patch_snapshot(
    outbound_tx: &mpsc::UnboundedSender<Message>,
    agent_id: &str,
    hostname: &str,
    boot_session_id: &str,
) -> Result<usize> {
    let payload = collect_full_snapshot_update(agent_id, hostname, boot_session_id).await?;
    let pending_update_count = snapshot_pending_update_count(&payload);
    send_envelope(outbound_tx, "full_snapshot", payload)?;
    info!(
        pending_update_count,
        "post-patch full_snapshot queued for websocket send"
    );
    Ok(pending_update_count)
}

pub(crate) fn snapshot_pending_update_count<T: Serialize>(payload: &T) -> usize {
    let Ok(value) = serde_json::to_value(payload) else {
        return 0;
    };
    let array_paths: [&[&str]; 8] = [
        &[
            "snapshot",
            "collection",
            "operating_system",
            "updates",
            "windows_update",
            "pending_updates",
        ],
        &[
            "collection",
            "operating_system",
            "updates",
            "windows_update",
            "pending_updates",
        ],
        &[
            "snapshot",
            "collection",
            "operatingSystem",
            "updates",
            "windowsUpdate",
            "pendingUpdates",
        ],
        &[
            "collection",
            "operatingSystem",
            "updates",
            "windowsUpdate",
            "pendingUpdates",
        ],
        &[
            "snapshot",
            "collection",
            "software",
            "windows_updates",
            "pending_updates",
        ],
        &[
            "collection",
            "software",
            "windows_updates",
            "pending_updates",
        ],
        &[
            "snapshot",
            "collection",
            "software",
            "windowsUpdates",
            "pendingUpdates",
        ],
        &["collection", "software", "windowsUpdates", "pendingUpdates"],
    ];

    for path in array_paths {
        if let Some(updates) = snapshot_value_at_path(&value, path).and_then(Value::as_array) {
            return updates.len();
        }
    }

    let count_paths: [&[&str]; 8] = [
        &[
            "snapshot",
            "collection",
            "operating_system",
            "updates",
            "windows_update",
            "pending_count",
        ],
        &[
            "collection",
            "operating_system",
            "updates",
            "windows_update",
            "pending_count",
        ],
        &[
            "snapshot",
            "collection",
            "operatingSystem",
            "updates",
            "windowsUpdate",
            "pendingCount",
        ],
        &[
            "collection",
            "operatingSystem",
            "updates",
            "windowsUpdate",
            "pendingCount",
        ],
        &[
            "snapshot",
            "collection",
            "software",
            "windows_updates",
            "pending_count",
        ],
        &["collection", "software", "windows_updates", "pending_count"],
        &[
            "snapshot",
            "collection",
            "software",
            "windowsUpdates",
            "pendingCount",
        ],
        &["collection", "software", "windowsUpdates", "pendingCount"],
    ];

    for path in count_paths {
        if let Some(count) = snapshot_value_at_path(&value, path).and_then(Value::as_u64) {
            return count as usize;
        }
    }

    0
}

fn snapshot_value_at_path<'a>(value: &'a Value, path: &[&str]) -> Option<&'a Value> {
    let mut current = value;
    for part in path {
        current = current.get(*part)?;
    }
    Some(current)
}

fn send_patch_job_update(
    outbound_tx: &mpsc::UnboundedSender<Message>,
    job_id: &str,
    status: &str,
    evidence: Value,
) -> Result<()> {
    send_envelope(
        outbound_tx,
        "patch_job_update",
        PatchJobUpdatePayload {
            job_id: job_id.to_string(),
            status: status.to_string(),
            step_index: 0,
            evidence,
        },
    )
}

fn send_patch_job_progress(
    reporter: Option<&PatchProgressReporter>,
    job: &PatchRemediationJob,
    phase: &'static str,
    updates: &[Value],
    summary: Value,
    snapshot: &PatchProgressSnapshot,
) {
    send_patch_job_progress_with_status(
        reporter, job, "running", phase, updates, summary, snapshot, None,
    );
}

struct PatchProgressHeartbeat {
    stop_tx: Option<std::sync::mpsc::Sender<()>>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl Drop for PatchProgressHeartbeat {
    fn drop(&mut self) {
        if let Some(stop_tx) = self.stop_tx.take() {
            let _ = stop_tx.send(());
        }
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn start_patch_job_progress_heartbeat(
    reporter: Option<&PatchProgressReporter>,
    job: &PatchRemediationJob,
    phase: &'static str,
    updates: &[Value],
    summary: Value,
    snapshot: &PatchProgressSnapshot,
) -> Option<PatchProgressHeartbeat> {
    let reporter = reporter.cloned()?;
    let job = job.clone();
    let updates = updates.to_vec();
    let snapshot = snapshot.clone();
    let (stop_tx, stop_rx) = std::sync::mpsc::channel();
    let handle = std::thread::spawn(move || loop {
        match stop_rx.recv_timeout(PATCH_JOB_PROGRESS_HEARTBEAT_INTERVAL) {
            Ok(_) | Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => send_patch_job_progress(
                Some(&reporter),
                &job,
                phase,
                &updates,
                summary.clone(),
                &snapshot,
            ),
        }
    });
    Some(PatchProgressHeartbeat {
        stop_tx: Some(stop_tx),
        handle: Some(handle),
    })
}

fn send_patch_job_progress_with_status(
    reporter: Option<&PatchProgressReporter>,
    job: &PatchRemediationJob,
    status: &'static str,
    phase: &'static str,
    updates: &[Value],
    summary: Value,
    snapshot: &PatchProgressSnapshot,
    error: Option<&str>,
) {
    let Some(reporter) = reporter else {
        return;
    };
    let current_update = snapshot
        .current_update_index
        .and_then(|index| updates.get(index.max(0) as usize))
        .map(|update| {
            json!({
                "updateKey": update.get("updateKey").cloned().unwrap_or(Value::Null),
                "title": update.get("title").cloned().unwrap_or(Value::Null),
                "kbArticle": update.get("kbArticle").cloned().unwrap_or(Value::Null),
                "index": snapshot.current_update_index
            })
        })
        .unwrap_or(Value::Null);
    let payload = json!({
        "schemaVersion": 1,
        "eventType": "patch.install.progress",
        "organizationId": job.organization_id,
        "agentId": job.agent_id,
        "jobId": reporter.job_id,
        "commandId": reporter.command_id,
        "status": status,
        "phase": phase,
        "reportedAt": Utc::now().to_rfc3339(),
        "overallPercent": snapshot.overall_percent.clamp(0, 100),
        "phasePercent": snapshot.phase_percent.clamp(0, 100),
        "currentUpdateIndex": snapshot.current_update_index,
        "currentUpdatePercent": snapshot.current_update_percent.map(|value| value.clamp(0, 100)),
        "currentUpdate": current_update,
        "updates": updates,
        "summary": summary,
        "error": error
    });
    if let Err(error) = send_envelope(&reporter.outbound_tx, "patch_job_progress", payload) {
        warn!(%error, job_id = %reporter.job_id, "failed to queue patch progress update");
    }
}

#[cfg(target_os = "macos")]
fn report_macos_update_account_status(
    reporter: Option<&PatchProgressReporter>,
    job: &PatchRemediationJob,
    status: talos_protocol::MacosUpdateAccountStatus,
) {
    let Some(reporter) = reporter else {
        return;
    };
    if let Err(error) = send_envelope(
        &reporter.outbound_tx,
        "macos_update_account_status",
        MacosUpdateAccountStatusPayload {
            agent_id: job.agent_id.clone(),
            status,
        },
    ) {
        warn!(%error, job_id = %reporter.job_id, "failed to queue macOS update account status");
    }
}

fn patch_scan_progress_payload(
    organization_id: &str,
    agent_id: &str,
    operation_id: &str,
    status: &str,
    pending_update_count: Option<usize>,
    error: Option<String>,
) -> Value {
    json!({
        "schemaVersion": 1,
        "eventType": "patch.scan.progress",
        "organizationId": organization_id,
        "agentId": agent_id,
        "jobId": operation_id,
        "commandId": operation_id,
        "status": status,
        "phase": "scanning",
        "reportedAt": Utc::now().to_rfc3339(),
        "overallPercent": if status == "running" { 0 } else { 100 },
        "phasePercent": if status == "running" { 0 } else { 100 },
        "currentUpdateIndex": Value::Null,
        "currentUpdatePercent": Value::Null,
        "currentUpdate": Value::Null,
        "updates": [],
        "summary": {
            "matched": 0,
            "downloaded": 0,
            "installed": 0,
            "failed": if status == "failed" { 1 } else { 0 },
            "skipped": 0,
            "rebootRequired": false,
            "pendingUpdates": pending_update_count,
            "snapshotRequested": true
        },
        "error": error
    })
}

fn send_patch_scan_progress(
    outbound_tx: &mpsc::UnboundedSender<Message>,
    organization_id: &str,
    agent_id: &str,
    operation_id: &str,
    status: &str,
    pending_update_count: Option<usize>,
    error: Option<String>,
) {
    let payload = patch_scan_progress_payload(
        organization_id,
        agent_id,
        operation_id,
        status,
        pending_update_count,
        error,
    );
    if let Err(error) = send_envelope(outbound_tx, "patch_job_progress", payload) {
        warn!(%error, operation_id, "failed to queue patch scan progress update");
    }
}

fn send_envelope<T: Serialize>(
    outbound_tx: &mpsc::UnboundedSender<Message>,
    message_type: &'static str,
    data: T,
) -> Result<()> {
    let envelope = OutgoingEnvelope { message_type, data };
    let text = serde_json::to_string(&envelope).context("serialize patch envelope")?;
    outbound_tx
        .send(Message::Text(text))
        .map_err(|_| anyhow::anyhow!("websocket outbound channel closed"))
}

#[allow(dead_code)]
pub(crate) fn execute_patch_job_blocking(job: PatchRemediationJob) -> PatchExecutionOutcome {
    execute_patch_job_blocking_with_progress(job, None)
}

pub(crate) fn execute_patch_job_blocking_with_progress(
    job: PatchRemediationJob,
    progress_reporter: Option<PatchProgressReporter>,
) -> PatchExecutionOutcome {
    #[cfg(target_os = "windows")]
    {
        match execute_patch_job_windows(&job, progress_reporter.as_ref()) {
            Ok(outcome) => outcome,
            Err(error) => failed_outcome(
                &job,
                "failed",
                format!("Windows patch execution failed: {error:#}"),
                Vec::new(),
            ),
        }
    }

    #[cfg(not(target_os = "windows"))]
    {
        #[cfg(target_os = "macos")]
        {
            match execute_patch_job_macos(&job, progress_reporter.as_ref()) {
                Ok(outcome) => outcome,
                Err(error) => failed_outcome(
                    &job,
                    "failed",
                    format!("macOS softwareupdate patch execution failed: {error:#}"),
                    Vec::new(),
                ),
            }
        }

        #[cfg(target_os = "linux")]
        {
            match detect_linux_patch_package_manager() {
                Ok(LinuxPatchPackageManager::Apt) => {
                    match execute_patch_job_linux_apt(&job, progress_reporter.as_ref()) {
                        Ok(outcome) => outcome,
                        Err(error) => failed_outcome(
                            &job,
                            "failed",
                            format!("Linux apt patch execution failed: {error:#}"),
                            Vec::new(),
                        ),
                    }
                }
                Ok(manager @ (LinuxPatchPackageManager::Dnf | LinuxPatchPackageManager::Yum)) => {
                    match execute_patch_job_linux_rpm(&job, progress_reporter.as_ref(), manager) {
                        Ok(outcome) => outcome,
                        Err(error) => failed_outcome(
                            &job,
                            "failed",
                            format!(
                                "Linux {} patch execution failed: {error:#}",
                                manager.label()
                            ),
                            Vec::new(),
                        ),
                    }
                }
                Err(error) => failed_outcome(
                    &job,
                    "unsupported_platform",
                    format!(
                        "Patch execution is only supported on Windows, Debian/Ubuntu apt, and Fedora/RHEL dnf/yum workers: {error:#}"
                    ),
                    Vec::new(),
                ),
            }
        }

        #[cfg(all(not(target_os = "linux"), not(target_os = "macos")))]
        {
            failed_outcome(
                &job,
                "unsupported_platform",
                "Patch execution is only supported on Windows, macOS softwareupdate, Debian/Ubuntu apt, and Fedora/RHEL dnf/yum workers"
                    .to_string(),
                Vec::new(),
            )
        }
    }
}

fn failed_outcome(
    job: &PatchRemediationJob,
    phase: &str,
    error: String,
    updates: Vec<Value>,
) -> PatchExecutionOutcome {
    PatchExecutionOutcome {
        status: "failed",
        evidence: json!({
            "phase": phase,
            "job": job_summary(job),
            "updates": updates,
            "summary": empty_summary(),
            "error": error
        }),
        force_reboot_after_report: false,
    }
}

fn job_summary(job: &PatchRemediationJob) -> Value {
    json!({
        "id": job.id,
        "organizationId": job.organization_id,
        "agentId": job.agent_id,
        "intentId": job.intent_id,
        "status": job.status,
        "dedupeKey": job.dedupe_key,
        "requestedAt": job.requested_at,
        "startedAt": job.started_at,
        "finishedAt": job.finished_at,
        "stepCount": job.steps.len(),
        "steps": job.steps.iter().map(|step| json!({
            "id": step.id,
            "stepIndex": step.step_index,
            "command": step.command,
            "status": step.status,
            "evidence": step.evidence,
            "startedAt": step.started_at,
            "finishedAt": step.finished_at
        })).collect::<Vec<_>>()
    })
}

fn empty_summary() -> Value {
    json!({
        "matched": 0,
        "downloaded": 0,
        "installed": 0,
        "failed": 0,
        "skipped": 0,
        "rebootRequired": false
    })
}

fn requested_update_keys(job: &PatchRemediationJob) -> HashSet<String> {
    requested_update_key_list(job).into_iter().collect()
}

fn requested_update_key_list(job: &PatchRemediationJob) -> Vec<String> {
    job.metadata
        .get("updateKeys")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn reboot_behavior(job: &PatchRemediationJob) -> String {
    if is_legacy_download_job(job) {
        return "suppress".to_string();
    }
    job.metadata
        .get("rebootBehavior")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("allow")
        .to_string()
}

fn is_legacy_download_job(job: &PatchRemediationJob) -> bool {
    job.metadata
        .get("downloadOnly")
        .and_then(Value::as_bool)
        .unwrap_or(false)
        || job
            .metadata
            .get("mode")
            .and_then(Value::as_str)
            .map(str::trim)
            .map(|value| {
                value.eq_ignore_ascii_case("download")
                    || value.eq_ignore_ascii_case("download_only")
            })
            .unwrap_or(false)
}

#[cfg_attr(target_os = "macos", allow(dead_code))]
fn is_download_only_job(_job: &PatchRemediationJob) -> bool {
    false
}

pub(crate) fn normalize_patch_text(value: &str) -> String {
    value
        .trim()
        .to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

pub(crate) fn build_patch_update_key(title: &str, kb_article: Option<&str>) -> String {
    format!(
        "{}|{}",
        normalize_patch_text(title),
        normalize_patch_text(kb_article.unwrap_or(""))
    )
}

#[cfg(any(target_os = "macos", test))]
pub(crate) fn macos_os_update_version_parts(title: &str) -> Option<Vec<u32>> {
    let normalized = normalize_patch_text(title);
    if !(normalized.starts_with("macos ") || normalized.starts_with("mac os ")) {
        return None;
    }
    if normalized.contains("security") || normalized.contains("critical") {
        return None;
    }

    normalized.split_whitespace().find_map(|part| {
        let value = part.trim_matches(|c: char| !c.is_ascii_digit() && c != '.');
        if value.is_empty() || !value.chars().any(|c| c.is_ascii_digit()) {
            return None;
        }
        let parts = value
            .split('.')
            .map(str::trim)
            .map(str::parse::<u32>)
            .collect::<Result<Vec<_>, _>>()
            .ok()?;
        if parts.is_empty() {
            None
        } else {
            Some(parts)
        }
    })
}

#[cfg(any(target_os = "macos", test))]
pub(crate) fn compare_patch_version_parts(left: &[u32], right: &[u32]) -> std::cmp::Ordering {
    let len = left.len().max(right.len());
    for index in 0..len {
        let left_part = left.get(index).copied().unwrap_or(0);
        let right_part = right.get(index).copied().unwrap_or(0);
        match left_part.cmp(&right_part) {
            std::cmp::Ordering::Equal => {}
            ordering => return ordering,
        }
    }
    std::cmp::Ordering::Equal
}

fn parse_update_key(value: &str) -> PatchUpdateIdentity {
    let (title, kb) = value.split_once('|').unwrap_or((value, ""));
    PatchUpdateIdentity {
        update_key: value.to_string(),
        title_norm: normalize_patch_text(title),
        kb_norm: normalize_patch_text(kb),
    }
}

fn update_matches_requested(update: &PatchUpdateIdentity, requested: &HashSet<String>) -> bool {
    if requested.is_empty() || requested.contains(&update.update_key) {
        return true;
    }

    requested
        .iter()
        .map(|value| parse_update_key(value))
        .any(|candidate| {
            candidate.title_norm == update.title_norm
                && (candidate.kb_norm.is_empty()
                    || update.kb_norm.is_empty()
                    || candidate.kb_norm == update.kb_norm)
        })
}

async fn refresh_patch_metadata_before_scan() -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        tokio::task::spawn_blocking(|| {
            run_macos_softwareupdate_blocking(&["-l"], "softwareupdate scan").map(|_| ())
        })
        .await
        .context("macOS patch metadata refresh task failed")?
    }

    #[cfg(target_os = "linux")]
    {
        tokio::task::spawn_blocking(refresh_linux_patch_metadata_blocking)
            .await
            .context("Linux patch metadata refresh task failed")?
    }

    #[cfg(all(not(target_os = "linux"), not(target_os = "macos")))]
    {
        Ok(())
    }
}

#[cfg(target_os = "macos")]
fn execute_patch_job_macos(
    job: &PatchRemediationJob,
    progress_reporter: Option<&PatchProgressReporter>,
) -> Result<PatchExecutionOutcome> {
    let behavior = reboot_behavior(job);
    let download_only = job
        .metadata
        .get("downloadOnly")
        .and_then(Value::as_bool)
        .unwrap_or(false)
        || job
            .metadata
            .get("mode")
            .and_then(Value::as_str)
            .map(str::trim)
            .map(|value| {
                value.eq_ignore_ascii_case("download")
                    || value.eq_ignore_ascii_case("download_only")
            })
            .unwrap_or(false);
    let requested = requested_update_keys(job);
    let candidates = query_macos_patch_candidates_blocking()?;
    let selected: Vec<MacosPatchCandidate> = candidates
        .iter()
        .filter(|candidate| update_matches_requested(&candidate.identity, &requested))
        .cloned()
        .collect();
    let skipped = candidates.len().saturating_sub(selected.len());
    let mut evidence_updates: Vec<Value> = candidates
        .iter()
        .map(|candidate| macos_candidate_evidence(candidate, selected.contains(candidate)))
        .collect();
    append_missing_macos_requested_update_evidence(&mut evidence_updates, &candidates, &requested);

    if selected.is_empty() {
        return Ok(PatchExecutionOutcome {
            status: "completed",
            evidence: json!({
                "phase": "completed",
                "job": job_summary(job),
                "updates": evidence_updates,
                "summary": patch_summary(0, 0, 0, 0, skipped, false),
                "error": null,
            }),
            force_reboot_after_report: false,
        });
    }

    let searching_snapshot = PatchProgressSnapshot {
        phase: "searching",
        overall_percent: 10,
        phase_percent: 100,
        current_update_index: None,
        current_update_percent: None,
    };
    send_patch_job_progress(
        progress_reporter,
        job,
        "searching",
        &macos_update_progress_values(&selected, "queued"),
        patch_summary(selected.len(), 0, 0, 0, skipped, false),
        &searching_snapshot,
    );

    let labels: Vec<&str> = selected
        .iter()
        .map(|candidate| candidate.label.as_str())
        .collect();
    let preflight_sw_vers = macos_sw_vers_evidence();
    let storage_preflight = match macos_storage_preflight(&selected) {
        Ok(evidence) => evidence,
        Err(failure) => {
            return Ok(macos_failed_outcome(
                job,
                progress_reporter,
                &selected,
                &mut evidence_updates,
                skipped,
                &failure.code,
                &failure.message,
                failure.evidence,
            ));
        }
    };
    let install_credential = if download_only {
        None
    } else {
        match talos_worker::macos_update_account::credential_for_softwareupdate() {
            Ok(credential) => credential,
            Err(failure) => {
                report_macos_update_account_status(progress_reporter, job, failure.status.clone());
                return Ok(macos_failed_outcome(
                    job,
                    progress_reporter,
                    &selected,
                    &mut evidence_updates,
                    skipped,
                    &failure.code,
                    &failure.message,
                    json!({
                        "code": failure.code,
                        "message": failure.message,
                        "accountStatus": failure.status,
                    }),
                ));
            }
        }
    };
    let mut downloaded = 0usize;
    let mut installed = 0usize;
    let mut failed = 0usize;
    let mut final_phase = "completed";
    let mut error = Value::Null;
    let mut failure_code: Option<String> = None;
    let mut error_message: Option<String> = None;

    if download_only {
        let downloading_updates = macos_update_progress_values(&selected, "downloading");
        let downloading_summary = patch_summary(selected.len(), 0, 0, 0, skipped, false);
        let downloading_snapshot = PatchProgressSnapshot {
            phase: "downloading",
            overall_percent: 35,
            phase_percent: 0,
            current_update_index: Some(0),
            current_update_percent: None,
        };
        send_patch_job_progress(
            progress_reporter,
            job,
            "downloading",
            &downloading_updates,
            downloading_summary.clone(),
            &downloading_snapshot,
        );
        let download_heartbeat = start_patch_job_progress_heartbeat(
            progress_reporter,
            job,
            "downloading",
            &downloading_updates,
            downloading_summary,
            &downloading_snapshot,
        );
        let args = macos_softwareupdate_label_args("-d", labels.iter().copied());
        match run_macos_softwareupdate_blocking(&args, "softwareupdate download") {
            Ok(_) => downloaded = selected.len(),
            Err(err) => {
                failed = selected.len();
                final_phase = "failed";
                let message = format!("{err:#}");
                failure_code = Some(classify_macos_softwareupdate_error(&message));
                error_message = Some(message.clone());
                error = json!(message);
            }
        }
        drop(download_heartbeat);
    } else {
        let installing_updates = macos_update_progress_values(&selected, "installing");
        let installing_summary = patch_summary(selected.len(), 0, 0, 0, skipped, false);
        let installing_snapshot = PatchProgressSnapshot {
            phase: "installing",
            overall_percent: 45,
            phase_percent: 0,
            current_update_index: Some(0),
            current_update_percent: None,
        };
        send_patch_job_progress(
            progress_reporter,
            job,
            "installing",
            &installing_updates,
            installing_summary.clone(),
            &installing_snapshot,
        );
        let install_heartbeat = start_patch_job_progress_heartbeat(
            progress_reporter,
            job,
            "installing",
            &installing_updates,
            installing_summary,
            &installing_snapshot,
        );
        let args = if let Some(credential) = install_credential.as_ref() {
            macos_softwareupdate_install_label_args_with_owner(
                &credential.username,
                labels.iter().copied(),
            )
        } else {
            macos_softwareupdate_install_label_args(labels.iter().copied())
        };
        match run_macos_softwareupdate_blocking_with_stdin(
            &args,
            "softwareupdate install",
            install_credential
                .as_ref()
                .map(|credential| format!("{}\n", credential.password)),
        ) {
            Ok(_) => {
                downloaded = selected.len();
                installed = selected.len();
            }
            Err(err) => {
                failed = selected.len();
                final_phase = "failed";
                let message = format!("{err:#}");
                failure_code = Some(classify_macos_softwareupdate_error(&message));
                error_message = Some(message.clone());
                error = json!(message);
            }
        }
        drop(install_heartbeat);
    }

    for update in &mut evidence_updates {
        if update
            .get("selected")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            update["downloaded"] = json!(downloaded > 0);
            update["installed"] = json!(!download_only && installed > 0);
            update["result"] = if failed > 0 {
                json!("failed")
            } else if download_only {
                json!("downloaded")
            } else {
                json!("installed")
            };
            if let Some(code) = failure_code.as_ref() {
                update["resultCode"] = json!(code);
            }
        }
    }

    let reboot_required = macos_reboot_required_for_outcome(&selected, installed);
    let status = if failed == 0 { "completed" } else { "failed" };
    let summary = patch_summary(
        selected.len(),
        downloaded,
        installed,
        failed,
        skipped,
        reboot_required,
    );
    let final_updates = if download_only {
        macos_update_progress_values(&selected, if failed == 0 { "downloaded" } else { "failed" })
    } else {
        macos_update_progress_values(&selected, if failed == 0 { "installed" } else { "failed" })
    };
    let final_snapshot = PatchProgressSnapshot {
        phase: final_phase,
        overall_percent: if failed == 0 { 100 } else { 95 },
        phase_percent: 100,
        current_update_index: None,
        current_update_percent: None,
    };
    send_patch_job_progress_with_status(
        progress_reporter,
        job,
        status,
        final_phase,
        &final_updates,
        summary.clone(),
        &final_snapshot,
        if failed == 0 {
            None
        } else {
            error_message.as_deref().or_else(|| error.as_str())
        },
    );

    Ok(PatchExecutionOutcome {
        status,
        evidence: json!({
            "phase": final_phase,
            "job": job_summary(job),
            "updates": evidence_updates.to_vec(),
            "summary": summary,
            "error": error,
            "failureCode": failure_code,
            "preflight": {
                "swVers": preflight_sw_vers,
                "storage": storage_preflight,
                "accountStatus": install_credential.as_ref().map(|credential| credential.status.clone()),
            },
        }),
        force_reboot_after_report: reboot_required && behavior == "force" && failed == 0,
    })
}

#[cfg(any(test, target_os = "macos"))]
fn macos_reboot_required_for_outcome(selected: &[MacosPatchCandidate], installed: usize) -> bool {
    installed > 0 && selected.iter().any(|candidate| candidate.requires_reboot)
}

#[cfg(target_os = "macos")]
fn query_macos_patch_candidates_blocking() -> Result<Vec<MacosPatchCandidate>> {
    let stdout = run_macos_softwareupdate_blocking(&["-l"], "softwareupdate list")?;
    Ok(parse_macos_softwareupdate_candidates(&stdout))
}

#[cfg(any(test, target_os = "macos"))]
fn macos_softwareupdate_label_args<'a>(
    operation: &'static str,
    labels: impl IntoIterator<Item = &'a str>,
) -> Vec<&'a str> {
    let mut args = vec![operation, "--"];
    args.extend(labels);
    args
}

#[cfg(any(test, target_os = "macos"))]
fn macos_softwareupdate_install_label_args<'a>(
    labels: impl IntoIterator<Item = &'a str>,
) -> Vec<&'a str> {
    let mut args = vec!["--agree-to-license", "-i", "--"];
    args.extend(labels);
    args
}

#[cfg(any(test, target_os = "macos"))]
fn macos_softwareupdate_install_label_args_with_owner<'a>(
    username: &'a str,
    labels: impl IntoIterator<Item = &'a str>,
) -> Vec<&'a str> {
    let mut args = vec![
        "--user",
        username,
        "--stdinpass",
        "--agree-to-license",
        "-i",
        "--",
    ];
    args.extend(labels);
    args
}

#[cfg(target_os = "macos")]
fn run_macos_softwareupdate_blocking(args: &[&str], description: &str) -> Result<String> {
    run_macos_softwareupdate_blocking_with_stdin(args, description, None)
}

#[cfg(target_os = "macos")]
fn run_macos_softwareupdate_blocking_with_stdin(
    args: &[&str],
    description: &str,
    stdin_text: Option<String>,
) -> Result<String> {
    let _guard = macos_softwareupdate_lock()
        .lock()
        .map_err(|_| anyhow::anyhow!("macOS softwareupdate lock poisoned"))?;
    let timeout = patch_command_timeout_from_env_value(
        std::env::var("RMM_PATCH_COMMAND_TIMEOUT_SECS")
            .ok()
            .as_deref(),
    );
    let secrets = macos_stdin_secret_values(stdin_text.as_deref());
    let stdout_path = macos_command_output_temp_path("stdout");
    let stderr_path = macos_command_output_temp_path("stderr");
    let stdout_file = create_macos_command_output_file(&stdout_path)
        .with_context(|| format!("create stdout capture for {description}"))?;
    let stderr_file = create_macos_command_output_file(&stderr_path)
        .with_context(|| format!("create stderr capture for {description}"))?;
    let mut command = std::process::Command::new("/usr/sbin/softwareupdate");
    command
        .args(args)
        .stdout(Stdio::from(stdout_file))
        .stderr(Stdio::from(stderr_file));
    if stdin_text.is_some() {
        command.stdin(Stdio::piped());
    }
    let mut child = command
        .spawn()
        .with_context(|| format!("spawn {description}"))?;
    if let Some(stdin_text) = stdin_text.as_deref() {
        if let Some(stdin) = child.stdin.as_mut() {
            stdin
                .write_all(stdin_text.as_bytes())
                .with_context(|| format!("write stdin for {description}"))?;
        }
    }
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child
            .try_wait()
            .with_context(|| format!("poll {description}"))?
        {
            break status;
        }
        if started.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            let stdout = read_macos_command_output(&stdout_path);
            let stderr = read_macos_command_output(&stderr_path);
            cleanup_macos_command_output(&stdout_path, &stderr_path);
            let detail = redact_macos_secret_values(
                &macos_command_failure_detail(&stdout, &stderr),
                &secrets,
            );
            let diagnostics = redact_macos_secret_values(
                &collect_macos_softwareupdate_failure_diagnostics(),
                &secrets,
            );
            anyhow::bail!(
                "{description} timed out after {} seconds{}{}",
                timeout.as_secs(),
                if detail.is_empty() { "" } else { ": " },
                macos_append_diagnostics(&detail, &diagnostics)
            );
        }
        std::thread::sleep(Duration::from_millis(250));
    };

    let stdout = read_macos_command_output(&stdout_path);
    let stderr = read_macos_command_output(&stderr_path);
    cleanup_macos_command_output(&stdout_path, &stderr_path);
    let combined_output =
        combined_macos_softwareupdate_output(stdout.as_bytes(), stderr.as_bytes());
    if status.success() {
        return Ok(redact_macos_secret_values(&combined_output, &secrets));
    }
    let detail =
        redact_macos_secret_values(&macos_command_failure_detail(&stdout, &stderr), &secrets);
    let diagnostics = redact_macos_secret_values(
        &collect_macos_softwareupdate_failure_diagnostics(),
        &secrets,
    );
    anyhow::bail!(
        "{description} exited with {}{}{}",
        status,
        if detail.is_empty() { "" } else { ": " },
        macos_append_diagnostics(&detail, &diagnostics)
    );
}

#[cfg(target_os = "macos")]
fn macos_softwareupdate_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

#[cfg(target_os = "macos")]
fn macos_command_output_temp_path(kind: &str) -> std::path::PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "talos_macos_patch_{}_{}_{}_{}.log",
        kind,
        std::process::id(),
        nanos,
        Uuid::new_v4()
    ))
}

#[cfg(target_os = "macos")]
fn create_macos_command_output_file(path: &std::path::Path) -> Result<File> {
    OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .mode(0o600)
        .open(path)
        .with_context(|| format!("open {}", path.display()))
}

#[cfg(target_os = "macos")]
fn read_macos_command_output(path: &std::path::Path) -> String {
    let mut bytes = Vec::new();
    if let Ok(mut file) = File::open(path) {
        let _ = file.read_to_end(&mut bytes);
    }
    String::from_utf8_lossy(&bytes).to_string()
}

#[cfg(target_os = "macos")]
fn cleanup_macos_command_output(stdout_path: &std::path::Path, stderr_path: &std::path::Path) {
    let _ = std::fs::remove_file(stdout_path);
    let _ = std::fs::remove_file(stderr_path);
}

#[cfg(any(test, target_os = "macos"))]
fn macos_command_failure_detail(stdout: &str, stderr: &str) -> String {
    [stdout.trim(), stderr.trim()]
        .into_iter()
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(any(test, target_os = "macos"))]
fn macos_stdin_secret_values(stdin_text: Option<&str>) -> Vec<String> {
    stdin_text
        .into_iter()
        .flat_map(str::lines)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .collect()
}

#[cfg(any(test, target_os = "macos"))]
fn redact_macos_secret_values(value: &str, secrets: &[String]) -> String {
    secrets.iter().fold(value.to_string(), |redacted, secret| {
        if secret.is_empty() {
            redacted
        } else {
            redacted.replace(secret, "[redacted]")
        }
    })
}

#[cfg(target_os = "macos")]
fn macos_append_diagnostics(detail: &str, diagnostics: &str) -> String {
    let detail = detail.trim();
    let diagnostics = diagnostics.trim();
    if diagnostics.is_empty() {
        detail.to_string()
    } else if detail.is_empty() {
        format!("diagnostics:\n{diagnostics}")
    } else {
        format!("{detail}\n\ndiagnostics:\n{diagnostics}")
    }
}

#[cfg(target_os = "macos")]
fn collect_macos_softwareupdate_failure_diagnostics() -> String {
    let _ = run_macos_diagnostic_command(
        "/usr/sbin/softwareupdate",
        &["--dump-state"],
        Duration::from_secs(10),
    );
    let install_log = recent_install_log_softwareupdate_lines();
    let unified_log = run_macos_diagnostic_command(
        "/usr/bin/log",
        &[
            "show",
            "--style",
            "compact",
            "--last",
            "30m",
            "--info",
            "--predicate",
            r#"process == "softwareupdated" OR eventMessage CONTAINS[c] "softwareupdate" OR subsystem CONTAINS[c] "SoftwareUpdate""#,
        ],
        Duration::from_secs(15),
    )
    .unwrap_or_default();
    [("install.log", install_log), ("unified.log", unified_log)]
        .into_iter()
        .filter_map(|(name, value)| {
            let value = truncate_macos_diagnostic(&value);
            if value.trim().is_empty() {
                None
            } else {
                Some(format!("{name}:\n{value}"))
            }
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

#[cfg(target_os = "macos")]
fn run_macos_diagnostic_command(program: &str, args: &[&str], timeout: Duration) -> Result<String> {
    let mut child = std::process::Command::new(program)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("spawn diagnostic {program}"))?;
    let started = Instant::now();
    loop {
        if child
            .try_wait()
            .with_context(|| format!("poll diagnostic {program}"))?
            .is_some()
        {
            let output = child
                .wait_with_output()
                .with_context(|| format!("read diagnostic {program}"))?;
            return Ok(combined_macos_softwareupdate_output(
                &output.stdout,
                &output.stderr,
            ));
        }
        if started.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            return Ok(String::new());
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

#[cfg(target_os = "macos")]
fn recent_install_log_softwareupdate_lines() -> String {
    let Ok(text) = std::fs::read_to_string("/var/log/install.log") else {
        return String::new();
    };
    let mut lines = text
        .lines()
        .rev()
        .filter(|line| {
            let lower = line.to_ascii_lowercase();
            lower.contains("softwareupdate")
                || lower.contains("software update")
                || lower.contains("softwareupdated")
        })
        .take(80)
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    lines.reverse();
    lines.join("\n")
}

#[cfg(any(target_os = "macos", test))]
fn truncate_macos_diagnostic(value: &str) -> String {
    const LIMIT: usize = 12_000;
    if value.len() <= LIMIT {
        value.trim().to_string()
    } else {
        format!("{}...", truncate_at_char_boundary(value, LIMIT).trim())
    }
}

#[cfg(any(test, target_os = "macos"))]
fn truncate_at_char_boundary(value: &str, limit: usize) -> &str {
    if value.len() <= limit {
        return value;
    }
    let end = value
        .char_indices()
        .map(|(index, _)| index)
        .take_while(|index| *index <= limit)
        .last()
        .unwrap_or(0);
    &value[..end]
}

#[cfg(any(test, target_os = "macos"))]
fn combined_macos_softwareupdate_output(stdout: &[u8], stderr: &[u8]) -> String {
    let stdout = String::from_utf8_lossy(stdout);
    let stderr = String::from_utf8_lossy(stderr);
    if stderr.trim().is_empty() {
        stdout.to_string()
    } else if stdout.trim().is_empty() {
        stderr.to_string()
    } else {
        format!("{stdout}\n{stderr}")
    }
}

#[cfg(any(test, target_os = "macos"))]
fn parse_macos_softwareupdate_candidates(stdout: &str) -> Vec<MacosPatchCandidate> {
    let mut updates = Vec::new();
    let mut current_label: Option<String> = None;
    let mut current_fields: serde_json::Map<String, Value> = serde_json::Map::new();

    for raw_line in stdout.lines() {
        let line = raw_line.trim();
        if line.is_empty()
            || line
                .to_ascii_lowercase()
                .starts_with("software update tool")
        {
            continue;
        }
        if line.eq_ignore_ascii_case("No new software available.") {
            current_label = None;
            current_fields.clear();
            break;
        }
        if let Some(label) = parse_macos_label_line(line) {
            flush_macos_softwareupdate_candidate(
                &mut updates,
                &mut current_label,
                &mut current_fields,
            );
            current_label = Some(label.trim_end_matches(',').to_string());
            continue;
        }
        let lower_line = line.to_ascii_lowercase();
        if lower_line.starts_with("title:")
            || lower_line.contains("action:")
            || lower_line.contains("recommended:")
            || lower_line.contains("size:")
        {
            for (key, value) in parse_macos_colon_fields(line) {
                current_fields.insert(key, Value::String(value));
            }
        }
    }

    flush_macos_softwareupdate_candidate(&mut updates, &mut current_label, &mut current_fields);
    filter_latest_macos_os_update_candidates(updates)
}

#[cfg(any(test, target_os = "macos"))]
fn filter_latest_macos_os_update_candidates(
    candidates: Vec<MacosPatchCandidate>,
) -> Vec<MacosPatchCandidate> {
    let latest = candidates
        .iter()
        .filter_map(|candidate| macos_os_update_version_parts(&candidate.title))
        .max_by(|left, right| compare_patch_version_parts(left, right));
    let Some(latest) = latest else {
        return candidates;
    };

    candidates
        .into_iter()
        .filter(|candidate| {
            macos_os_update_version_parts(&candidate.title)
                .map(|version| {
                    compare_patch_version_parts(&version, &latest) == std::cmp::Ordering::Equal
                })
                .unwrap_or(true)
        })
        .collect()
}

#[cfg(any(test, target_os = "macos"))]
fn parse_macos_label_line(line: &str) -> Option<&str> {
    let trimmed = line.trim();
    let candidate = trimmed.strip_prefix('*').map(str::trim).unwrap_or(trimmed);
    let colon = candidate.find(':')?;
    let key = candidate[..colon].trim();
    if !key.eq_ignore_ascii_case("label") {
        return None;
    }
    let value = candidate[colon + 1..].trim().trim_end_matches(',').trim();
    if value.is_empty() {
        None
    } else {
        Some(value)
    }
}

#[cfg(any(test, target_os = "macos"))]
fn flush_macos_softwareupdate_candidate(
    updates: &mut Vec<MacosPatchCandidate>,
    current_label: &mut Option<String>,
    current_fields: &mut serde_json::Map<String, Value>,
) {
    let Some(label) = current_label.take() else {
        current_fields.clear();
        return;
    };
    let title = current_fields
        .get("title")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| label.clone());
    let recommended = current_fields
        .get("recommended")
        .and_then(Value::as_str)
        .map(|value| value.eq_ignore_ascii_case("yes") || value.eq_ignore_ascii_case("true"))
        .unwrap_or(false);
    let requires_reboot = current_fields
        .get("action")
        .and_then(Value::as_str)
        .map(|value| value.to_ascii_lowercase().contains("restart"))
        .unwrap_or(false);
    let identity = PatchUpdateIdentity {
        update_key: build_patch_update_key(&title, None),
        title_norm: normalize_patch_text(&title),
        kb_norm: String::new(),
    };

    updates.push(MacosPatchCandidate {
        label,
        title,
        version: current_fields
            .get("version")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        size: current_fields
            .get("size")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        recommended,
        requires_reboot,
        identity,
    });
    current_fields.clear();
}

#[cfg(any(test, target_os = "macos"))]
fn parse_macos_colon_fields(line: &str) -> Vec<(String, String)> {
    parse_known_colon_fields(line, &["Title", "Version", "Size", "Recommended", "Action"])
}

#[cfg(any(test, target_os = "macos"))]
fn parse_known_colon_fields(line: &str, keys: &[&str]) -> Vec<(String, String)> {
    let mut markers = keys
        .iter()
        .filter_map(|key| {
            let marker = format!("{key}:");
            find_ascii_case_insensitive(line, &marker).map(|offset| (offset, *key, marker.len()))
        })
        .collect::<Vec<_>>();
    markers.sort_by_key(|(offset, _, _)| *offset);

    markers
        .iter()
        .enumerate()
        .filter_map(|(index, (offset, key, marker_len))| {
            let value_start = offset + marker_len;
            let value_end = markers
                .get(index + 1)
                .map(|(next_offset, _, _)| *next_offset)
                .unwrap_or(line.len());
            let value = line[value_start..value_end]
                .trim()
                .trim_start_matches(',')
                .trim()
                .trim_end_matches(',')
                .trim()
                .to_string();
            if value.is_empty() {
                None
            } else {
                Some((key.to_ascii_lowercase().replace(' ', "_"), value))
            }
        })
        .collect()
}

#[cfg(any(test, target_os = "macos"))]
fn find_ascii_case_insensitive(haystack: &str, needle: &str) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack
        .as_bytes()
        .windows(needle.len())
        .position(|window| window.eq_ignore_ascii_case(needle.as_bytes()))
}

#[cfg(target_os = "macos")]
fn macos_update_progress_values(candidates: &[MacosPatchCandidate], state: &str) -> Vec<Value> {
    candidates
        .iter()
        .enumerate()
        .map(|(index, candidate)| {
            json!({
                "updateKey": candidate.identity.update_key.clone(),
                "title": candidate.title.clone(),
                "kbArticle": Value::Null,
                "index": index,
                "state": state,
                "percent": if matches!(state, "downloaded" | "installed" | "failed") {
                    100
                } else {
                    0
                }
            })
        })
        .collect()
}

#[cfg(any(test, target_os = "macos"))]
fn macos_candidate_evidence(candidate: &MacosPatchCandidate, selected: bool) -> Value {
    json!({
        "title": candidate.title,
        "name": candidate.title,
        "description": candidate.label,
        "label": candidate.label,
        "version": candidate.version,
        "size": candidate.size,
        "kbArticle": null,
        "updateKey": candidate.identity.update_key,
        "source": "softwareupdate",
        "matched": selected,
        "selected": selected,
        "recommended": candidate.recommended,
        "requiresReboot": candidate.requires_reboot,
        "downloaded": false,
        "installed": false,
        "result": if selected { "queued" } else { "skipped" },
        "resultCode": null,
        "hresult": null,
    })
}

#[cfg(any(test, target_os = "macos"))]
fn append_missing_macos_requested_update_evidence(
    evidence_updates: &mut Vec<Value>,
    candidates: &[MacosPatchCandidate],
    requested: &HashSet<String>,
) {
    for requested_key in requested {
        if candidates.iter().any(|candidate| {
            update_matches_requested(&candidate.identity, &HashSet::from([requested_key.clone()]))
        }) {
            continue;
        }
        evidence_updates.push(json!({
            "updateKey": requested_key,
            "title": null,
            "name": null,
            "description": null,
            "label": null,
            "version": null,
            "size": null,
            "kbArticle": null,
            "source": "softwareupdate",
            "matched": false,
            "selected": false,
            "recommended": false,
            "requiresReboot": false,
            "downloaded": false,
            "installed": false,
            "result": "not_found",
            "resultCode": null,
            "hresult": null,
        }));
    }
}

#[cfg(target_os = "macos")]
#[derive(Debug, Clone)]
struct MacosPreflightFailure {
    code: String,
    message: String,
    evidence: Value,
}

#[cfg(target_os = "macos")]
fn macos_failed_outcome(
    job: &PatchRemediationJob,
    progress_reporter: Option<&PatchProgressReporter>,
    selected: &[MacosPatchCandidate],
    evidence_updates: &mut [Value],
    skipped: usize,
    code: &str,
    message: &str,
    extra: Value,
) -> PatchExecutionOutcome {
    for update in evidence_updates.iter_mut() {
        if update
            .get("selected")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            update["downloaded"] = json!(false);
            update["installed"] = json!(false);
            update["result"] = json!("failed");
            update["resultCode"] = json!(code);
        }
    }
    let summary = patch_summary(selected.len(), 0, 0, selected.len(), skipped, false);
    let final_updates = macos_update_progress_values(selected, "failed");
    let final_snapshot = PatchProgressSnapshot {
        phase: "failed",
        overall_percent: 95,
        phase_percent: 100,
        current_update_index: None,
        current_update_percent: None,
    };
    send_patch_job_progress_with_status(
        progress_reporter,
        job,
        "failed",
        "failed",
        &final_updates,
        summary.clone(),
        &final_snapshot,
        Some(message),
    );
    PatchExecutionOutcome {
        status: "failed",
        evidence: json!({
            "phase": "failed",
            "job": job_summary(job),
            "updates": evidence_updates,
            "summary": summary,
            "error": message,
            "failureCode": code,
            "failure": extra,
        }),
        force_reboot_after_report: false,
    }
}

#[cfg(target_os = "macos")]
fn macos_storage_preflight(
    selected: &[MacosPatchCandidate],
) -> std::result::Result<Value, MacosPreflightFailure> {
    let selected_size = selected
        .iter()
        .filter_map(|candidate| candidate.size.as_deref())
        .filter_map(parse_macos_size_bytes)
        .sum::<u64>();
    let is_version_upgrade = selected.iter().any(|candidate| {
        let title = candidate.title.to_ascii_lowercase();
        title.contains("macos") || title.contains("mac os")
    });
    const GIB: u64 = 1024 * 1024 * 1024;
    let minimum = if is_version_upgrade {
        env_u64_gib("TALOS_MACOS_UPDATE_MIN_VERSION_UPGRADE_FREE_GIB", 25)
    } else {
        env_u64_gib("TALOS_MACOS_UPDATE_MIN_FREE_GIB", 15)
    }
    .saturating_mul(GIB);
    let buffer = env_u64_gib("TALOS_MACOS_UPDATE_FREE_BUFFER_GIB", 5).saturating_mul(GIB);
    let required = minimum.max(selected_size.saturating_add(buffer));
    let paths = ["/", "/System/Volumes/Data", "/Library/Updates"];
    let volumes = paths
        .iter()
        .map(|path| {
            let measurement = filesystem_available_bytes_for_path(path);
            let (measured_path, available, error) = match measurement {
                Ok((measured_path, available)) => (measured_path, available, None),
                Err(error) => (path.to_string(), 0, Some(error.to_string())),
            };
            json!({
                "path": path,
                "measuredPath": measured_path,
                "availableBytes": available,
                "requiredBytes": required,
                "ok": available >= required,
                "error": error,
            })
        })
        .collect::<Vec<_>>();
    let failed = volumes.iter().any(|volume| {
        volume
            .get("ok")
            .and_then(Value::as_bool)
            .map(|ok| !ok)
            .unwrap_or(true)
    });
    let evidence = json!({
        "code": "macos_storage_preflight",
        "selectedSizeBytes": selected_size,
        "requiredBytes": required,
        "minimumFreeBytes": minimum,
        "bufferBytes": buffer,
        "versionUpgrade": is_version_upgrade,
        "volumes": volumes,
    });
    if failed {
        Err(MacosPreflightFailure {
            code: "macos_low_storage".to_string(),
            message: format!(
                "macOS update requires at least {} GiB free on update volumes before Talos can install it.",
                required / 1024 / 1024 / 1024
            ),
            evidence,
        })
    } else {
        Ok(evidence)
    }
}

#[cfg(any(test, target_os = "macos"))]
fn parse_macos_size_bytes(raw: &str) -> Option<u64> {
    let text = raw.trim().replace(',', "");
    if text.is_empty() {
        return None;
    }
    let split_at = text
        .char_indices()
        .find(|(_, ch)| !(ch.is_ascii_digit() || *ch == '.'))
        .map(|(idx, _)| idx)
        .unwrap_or(text.len());
    let number = text[..split_at].trim().parse::<f64>().ok()?;
    let unit = text[split_at..].trim().to_ascii_lowercase();
    let multiplier = if unit.starts_with("kib") || unit == "k" || unit.starts_with("kb") {
        1024_f64
    } else if unit.starts_with("mib") || unit.starts_with("mb") || unit == "m" {
        1024_f64 * 1024_f64
    } else if unit.starts_with("gib") || unit.starts_with("gb") || unit == "g" {
        1024_f64 * 1024_f64 * 1024_f64
    } else if unit.starts_with("tib") || unit.starts_with("tb") || unit == "t" {
        1024_f64 * 1024_f64 * 1024_f64 * 1024_f64
    } else {
        1_f64
    };
    Some((number * multiplier).round() as u64)
}

#[cfg(target_os = "macos")]
fn env_u64_gib(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .unwrap_or(default)
}

#[cfg(target_os = "macos")]
fn filesystem_available_bytes_for_path(path: &str) -> Result<(String, u64)> {
    let measured_path = nearest_existing_path(path);
    let available = filesystem_available_bytes(&measured_path)?;
    Ok((measured_path.display().to_string(), available))
}

#[cfg(target_os = "macos")]
fn nearest_existing_path(path: &str) -> PathBuf {
    let mut current = PathBuf::from(path);
    loop {
        if current.exists() {
            return current;
        }
        if !current.pop() {
            return PathBuf::from("/");
        }
    }
}

#[cfg(target_os = "macos")]
fn filesystem_available_bytes(path: &Path) -> Result<u64> {
    let c_path =
        std::ffi::CString::new(path.as_os_str().as_bytes()).context("build statfs path")?;
    let mut stat = std::mem::MaybeUninit::<libc::statfs>::uninit();
    let rc = unsafe { libc::statfs(c_path.as_ptr(), stat.as_mut_ptr()) };
    if rc != 0 {
        return Err(std::io::Error::last_os_error())
            .with_context(|| format!("statfs {}", path.display()));
    }
    let stat = unsafe { stat.assume_init() };
    Ok((stat.f_bavail as u64).saturating_mul(stat.f_bsize as u64))
}

#[cfg(target_os = "macos")]
fn macos_sw_vers_evidence() -> Value {
    let output = std::process::Command::new("/usr/bin/sw_vers").output();
    match output {
        Ok(output) => {
            let text = String::from_utf8_lossy(&output.stdout);
            let mut product = serde_json::Map::new();
            for line in text.lines() {
                if let Some((key, value)) = line.split_once(':') {
                    product.insert(
                        key.trim().to_ascii_lowercase().replace(' ', "_"),
                        json!(value.trim()),
                    );
                }
            }
            Value::Object(product)
        }
        Err(error) => json!({ "error": error.to_string() }),
    }
}

#[cfg(target_os = "macos")]
fn classify_macos_softwareupdate_error(message: &str) -> String {
    let lower = message.to_ascii_lowercase();
    if lower.contains("not enough") || lower.contains("space") || lower.contains("storage") {
        "macos_low_storage"
    } else if lower.contains("password")
        || lower.contains("auth")
        || lower.contains("owner")
        || lower.contains("credential")
    {
        "macos_softwareupdate_auth_failed"
    } else if lower.contains("busy") || lower.contains("another") || lower.contains("locked") {
        "macos_softwareupdate_busy"
    } else if lower.contains("timed out") {
        "macos_softwareupdate_timeout"
    } else if lower.contains("download") {
        "macos_softwareupdate_download_failed"
    } else if lower.contains("install") {
        "macos_softwareupdate_install_failed"
    } else {
        "macos_softwareupdate_failed"
    }
    .to_string()
}

#[cfg(target_os = "macos")]
fn patch_summary(
    matched: usize,
    downloaded: usize,
    installed: usize,
    failed: usize,
    skipped: usize,
    reboot_required: bool,
) -> Value {
    json!({
        "matched": matched,
        "downloaded": downloaded,
        "installed": installed,
        "failed": failed,
        "skipped": skipped,
        "rebootRequired": reboot_required,
    })
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn parse_apt_upgradable_candidates(stdout: &str, reboot_required: bool) -> Vec<AptPatchCandidate> {
    let mut updates = Vec::new();
    let mut seen = HashSet::new();
    for line in stdout.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty()
            || trimmed.starts_with("Listing...")
            || trimmed.starts_with("WARNING:")
            || !trimmed.contains('/')
        {
            continue;
        }

        let columns = trimmed.split_whitespace().collect::<Vec<_>>();
        if columns.len() < 2 {
            continue;
        }
        let package_source = columns[0];
        let package = package_source.split('/').next().unwrap_or_default().trim();
        let source = package_source
            .split_once('/')
            .map(|(_, value)| value.trim().to_string())
            .filter(|value| !value.is_empty());
        let target_version = columns[1].trim();
        if package.is_empty()
            || target_version.is_empty()
            || !seen.insert(format!("{package}|{target_version}"))
        {
            continue;
        }

        let current_version = trimmed
            .split("[upgradable from:")
            .nth(1)
            .and_then(|rest| rest.split(']').next())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned);
        let architecture = columns
            .get(2)
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty() && !value.starts_with('['));
        let title = format!("{package} {target_version}");
        let identity = PatchUpdateIdentity {
            update_key: build_patch_update_key(&title, None),
            title_norm: normalize_patch_text(&title),
            kb_norm: String::new(),
        };
        updates.push(AptPatchCandidate {
            package: package.to_string(),
            source,
            target_version: target_version.to_string(),
            current_version,
            architecture,
            title,
            description: trimmed.to_string(),
            identity,
            requires_reboot: reboot_required,
        });
    }
    updates
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn requested_package_name_from_update_key(update_key: &str) -> Option<String> {
    let title = update_key
        .split_once('|')
        .map(|(title, _)| title)
        .unwrap_or(update_key);
    title
        .split_whitespace()
        .next()
        .map(normalize_patch_text)
        .filter(|value| !value.is_empty())
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn apt_candidate_matches_requested(
    candidate: &AptPatchCandidate,
    requested: &HashSet<String>,
) -> bool {
    if update_matches_requested(&candidate.identity, requested) {
        return true;
    }
    if requested.is_empty() {
        return true;
    }
    let package_norm = normalize_patch_text(&candidate.package);
    requested
        .iter()
        .filter_map(|value| requested_package_name_from_update_key(value))
        .any(|value| value == package_norm)
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn apt_package_specs(candidates: &[AptPatchCandidate]) -> Vec<String> {
    candidates
        .iter()
        .map(|candidate| format!("{}={}", candidate.package, candidate.target_version))
        .collect()
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn apt_get_update_args() -> Vec<String> {
    vec![
        "-o".to_string(),
        "DPkg::Lock::Timeout=300".to_string(),
        "update".to_string(),
    ]
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn apt_dpkg_safe_args() -> Vec<String> {
    vec![
        "-y".to_string(),
        "-o".to_string(),
        "DPkg::Lock::Timeout=300".to_string(),
        "-o".to_string(),
        "Dpkg::Options::=--force-confdef".to_string(),
        "-o".to_string(),
        "Dpkg::Options::=--force-confold".to_string(),
    ]
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn apt_get_download_args(package_specs: &[String]) -> Vec<String> {
    let mut args = apt_dpkg_safe_args();
    args.push("--download-only".to_string());
    args.push("install".to_string());
    args.push("--only-upgrade".to_string());
    args.extend(package_specs.iter().cloned());
    args
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn apt_get_install_args(package_specs: &[String]) -> Vec<String> {
    let mut args = apt_dpkg_safe_args();
    args.push("install".to_string());
    args.push("--only-upgrade".to_string());
    args.extend(package_specs.iter().cloned());
    args
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn apt_progress_summary(
    matched: usize,
    downloaded: usize,
    installed: usize,
    failed: usize,
    skipped: usize,
    reboot_required: bool,
) -> Value {
    json!({
        "matched": matched,
        "downloaded": downloaded,
        "installed": installed,
        "failed": failed,
        "skipped": skipped,
        "rebootRequired": reboot_required
    })
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn apt_update_progress_values(candidates: &[AptPatchCandidate], state: &str) -> Vec<Value> {
    candidates
        .iter()
        .enumerate()
        .map(|(index, candidate)| {
            json!({
                "updateKey": candidate.identity.update_key.clone(),
                "title": candidate.title.clone(),
                "kbArticle": Value::Null,
                "index": index,
                "state": state,
                "percent": if matches!(state, "downloaded" | "installed") { 100 } else { 0 }
            })
        })
        .collect()
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn apt_candidate_evidence(candidate: &AptPatchCandidate, matched: bool) -> Value {
    json!({
        "updateKey": candidate.identity.update_key.clone(),
        "title": candidate.title.clone(),
        "kbArticle": Value::Null,
        "matched": matched,
        "downloaded": false,
        "installed": false,
        "resultCode": Value::Null,
        "result": Value::Null,
        "hresult": Value::Null,
        "requiresReboot": candidate.requires_reboot,
        "package": candidate.package.clone(),
        "source": candidate.source.clone(),
        "targetVersion": candidate.target_version.clone(),
        "currentVersion": candidate.current_version.clone(),
        "architecture": candidate.architecture.clone(),
        "description": candidate.description.clone()
    })
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn mark_apt_evidence_updates(
    evidence_updates: &mut [Value],
    selected: &[AptPatchCandidate],
    downloaded: bool,
    installed: bool,
    result: &str,
    result_code: Option<i32>,
    error: Option<&str>,
) {
    let selected_keys = selected
        .iter()
        .map(|candidate| candidate.identity.update_key.clone())
        .collect::<HashSet<_>>();
    for update in evidence_updates {
        let selected = update
            .get("updateKey")
            .and_then(Value::as_str)
            .map(|key| selected_keys.contains(key))
            .unwrap_or(false);
        if !selected {
            continue;
        }
        update["downloaded"] = json!(downloaded);
        update["installed"] = json!(installed);
        update["result"] = json!(result);
        update["resultCode"] = result_code.map_or(Value::Null, |value| json!(value));
        update["hresult"] = result_code.map_or(Value::Null, |value| json!(value));
        update["error"] = error.map_or(Value::Null, |value| json!(value));
    }
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn apt_terminal_progress_values(
    selected: &[AptPatchCandidate],
    evidence_updates: &[Value],
) -> Vec<Value> {
    selected
        .iter()
        .enumerate()
        .map(|(index, candidate)| {
            let evidence = evidence_updates.iter().find(|update| {
                update
                    .get("updateKey")
                    .and_then(Value::as_str)
                    .map(|key| key == candidate.identity.update_key)
                    .unwrap_or(false)
            });
            let installed = evidence
                .and_then(|update| update.get("installed"))
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let downloaded = evidence
                .and_then(|update| update.get("downloaded"))
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let result = evidence
                .and_then(|update| update.get("result"))
                .and_then(Value::as_str)
                .unwrap_or_default();
            let state = if installed {
                "installed"
            } else if downloaded {
                "downloaded"
            } else if result.is_empty() {
                "queued"
            } else {
                "failed"
            };
            json!({
                "updateKey": candidate.identity.update_key.clone(),
                "title": candidate.title.clone(),
                "kbArticle": Value::Null,
                "index": index,
                "state": state,
                "percent": if matches!(state, "downloaded" | "installed") { 100 } else { 0 }
            })
        })
        .collect()
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn rpm_arch_suffixes() -> &'static [&'static str] {
    &[
        ".x86_64", ".noarch", ".aarch64", ".i686", ".armv7hl", ".ppc64le", ".s390x",
    ]
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn split_rpm_package_arch(value: &str) -> (String, Option<String>) {
    for suffix in rpm_arch_suffixes() {
        if let Some(package) = value.strip_suffix(suffix) {
            return (
                package.to_string(),
                Some(suffix.trim_start_matches('.').to_string()),
            );
        }
    }
    (value.to_string(), None)
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn rpm_package_requires_reboot(package: &str) -> bool {
    let package = normalize_patch_text(package);
    package == "kernel"
        || package.starts_with("kernel-")
        || package == "systemd"
        || package.starts_with("systemd-")
        || package == "glibc"
        || package.starts_with("glibc-")
        || package == "dbus"
        || package.starts_with("dbus-")
        || package == "rpm"
        || package.starts_with("rpm-")
        || package == "dnf"
        || package.starts_with("dnf-")
        || package == "dnf5"
        || package.starts_with("dnf5-")
        || package == "libdnf"
        || package.starts_with("libdnf")
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn parse_dnf_yum_check_update_candidates(
    stdout: &str,
    reboot_required: bool,
) -> Vec<RpmPatchCandidate> {
    let mut updates = Vec::new();
    let mut seen = HashSet::new();
    let mut in_obsoleting_packages = false;

    for line in stdout.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty()
            || trimmed.starts_with("Last metadata expiration check")
            || trimmed.starts_with("Loaded plugins:")
            || trimmed.eq_ignore_ascii_case("Available Upgrades")
        {
            continue;
        }
        if trimmed.eq_ignore_ascii_case("Obsoleting Packages") {
            in_obsoleting_packages = true;
            continue;
        }
        if in_obsoleting_packages {
            continue;
        }

        let columns = trimmed.split_whitespace().collect::<Vec<_>>();
        if columns.len() < 3 {
            continue;
        }

        let (package, architecture) = split_rpm_package_arch(columns[0].trim());
        let target_version = columns[1].trim();
        let source = columns[2].trim();
        if package.is_empty()
            || target_version.is_empty()
            || !seen.insert(format!("{package}|{target_version}"))
        {
            continue;
        }

        let title = format!("{package} {target_version}");
        let identity = PatchUpdateIdentity {
            update_key: build_patch_update_key(&title, None),
            title_norm: normalize_patch_text(&title),
            kb_norm: String::new(),
        };
        let package_requires_reboot = rpm_package_requires_reboot(&package);
        updates.push(RpmPatchCandidate {
            package,
            source: if source.is_empty() {
                None
            } else {
                Some(source.to_string())
            },
            target_version: target_version.to_string(),
            current_version: None,
            architecture,
            title,
            description: trimmed.to_string(),
            identity,
            requires_reboot: reboot_required || package_requires_reboot,
        });
    }

    updates
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn rpm_candidate_matches_requested(
    candidate: &RpmPatchCandidate,
    requested: &HashSet<String>,
) -> bool {
    if update_matches_requested(&candidate.identity, requested) {
        return true;
    }
    if requested.is_empty() {
        return true;
    }
    let package_norm = normalize_patch_text(&candidate.package);
    requested
        .iter()
        .filter_map(|value| requested_package_name_from_update_key(value))
        .any(|value| value == package_norm)
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn rpm_package_specs(candidates: &[RpmPatchCandidate]) -> Vec<String> {
    candidates
        .iter()
        .map(|candidate| candidate.package.clone())
        .collect()
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn rpm_install_package_specs(
    selected: &[RpmPatchCandidate],
    requested_update_keys: &[String],
) -> Vec<String> {
    let mut specs = Vec::new();
    let mut seen = HashSet::new();

    for candidate in selected {
        let key = normalize_patch_text(&candidate.package);
        if seen.insert(key) {
            specs.push(candidate.package.clone());
        }
    }

    for update_key in requested_update_keys {
        let Some(package) = requested_package_name_from_update_key(update_key) else {
            continue;
        };
        if seen.insert(package.clone()) {
            specs.push(package);
        }
    }

    specs
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn dnf_yum_makecache_args(manager: LinuxPatchPackageManager) -> Vec<String> {
    match manager {
        LinuxPatchPackageManager::Dnf => vec!["makecache".to_string(), "--refresh".to_string()],
        LinuxPatchPackageManager::Yum => vec!["makecache".to_string()],
        LinuxPatchPackageManager::Apt => vec!["update".to_string()],
    }
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn dnf_yum_check_update_args() -> Vec<String> {
    vec!["check-update".to_string()]
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn dnf_yum_download_args(package_specs: &[String]) -> Vec<String> {
    let mut args = vec![
        "-y".to_string(),
        "upgrade".to_string(),
        "--downloadonly".to_string(),
    ];
    args.extend(package_specs.iter().cloned());
    args
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn dnf_yum_install_args(package_specs: &[String]) -> Vec<String> {
    let mut args = vec!["-y".to_string(), "upgrade".to_string()];
    args.extend(package_specs.iter().cloned());
    args
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn rpm_update_progress_values(candidates: &[RpmPatchCandidate], state: &str) -> Vec<Value> {
    candidates
        .iter()
        .enumerate()
        .map(|(index, candidate)| {
            json!({
                "updateKey": candidate.identity.update_key.clone(),
                "title": candidate.title.clone(),
                "kbArticle": Value::Null,
                "index": index,
                "state": state,
                "percent": if matches!(state, "downloaded" | "installed") { 100 } else { 0 }
            })
        })
        .collect()
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn rpm_candidate_evidence(candidate: &RpmPatchCandidate, matched: bool) -> Value {
    json!({
        "updateKey": candidate.identity.update_key.clone(),
        "title": candidate.title.clone(),
        "kbArticle": Value::Null,
        "matched": matched,
        "downloaded": false,
        "installed": false,
        "resultCode": Value::Null,
        "result": Value::Null,
        "hresult": Value::Null,
        "requiresReboot": candidate.requires_reboot,
        "package": candidate.package.clone(),
        "source": candidate.source.clone(),
        "targetVersion": candidate.target_version.clone(),
        "currentVersion": candidate.current_version.clone(),
        "architecture": candidate.architecture.clone(),
        "description": candidate.description.clone()
    })
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn mark_rpm_evidence_updates(
    evidence_updates: &mut [Value],
    selected: &[RpmPatchCandidate],
    downloaded: bool,
    installed: bool,
    result: &str,
    result_code: Option<i32>,
    error: Option<&str>,
) {
    let selected_keys = selected
        .iter()
        .map(|candidate| candidate.identity.update_key.clone())
        .collect::<HashSet<_>>();
    for update in evidence_updates {
        let selected = update
            .get("updateKey")
            .and_then(Value::as_str)
            .map(|key| selected_keys.contains(key))
            .unwrap_or(false);
        if !selected {
            continue;
        }
        update["downloaded"] = json!(downloaded);
        update["installed"] = json!(installed);
        update["result"] = json!(result);
        update["resultCode"] = result_code.map_or(Value::Null, |value| json!(value));
        update["hresult"] = result_code.map_or(Value::Null, |value| json!(value));
        update["error"] = error.map_or(Value::Null, |value| json!(value));
    }
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn rpm_terminal_progress_values(
    selected: &[RpmPatchCandidate],
    evidence_updates: &[Value],
) -> Vec<Value> {
    selected
        .iter()
        .enumerate()
        .map(|(index, candidate)| {
            let evidence = evidence_updates.iter().find(|update| {
                update
                    .get("updateKey")
                    .and_then(Value::as_str)
                    .map(|key| key == candidate.identity.update_key)
                    .unwrap_or(false)
            });
            let installed = evidence
                .and_then(|update| update.get("installed"))
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let downloaded = evidence
                .and_then(|update| update.get("downloaded"))
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let result = evidence
                .and_then(|update| update.get("result"))
                .and_then(Value::as_str)
                .unwrap_or_default();
            let state = if installed {
                "installed"
            } else if downloaded {
                "downloaded"
            } else if result.is_empty() {
                "queued"
            } else {
                "failed"
            };
            json!({
                "updateKey": candidate.identity.update_key.clone(),
                "title": candidate.title.clone(),
                "kbArticle": Value::Null,
                "index": index,
                "state": state,
                "percent": if matches!(state, "downloaded" | "installed") { 100 } else { 0 }
            })
        })
        .collect()
}

#[cfg(target_os = "linux")]
#[derive(Debug)]
struct AptCommandOutput {
    status_code: Option<i32>,
    stdout: String,
    stderr: String,
}

#[cfg(target_os = "linux")]
fn linux_command_available(command: &str) -> bool {
    std::process::Command::new("sh")
        .args(["-c", &format!("command -v {command} >/dev/null 2>&1")])
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

#[cfg(target_os = "linux")]
fn ensure_linux_patch_root() -> Result<()> {
    if unsafe { libc::geteuid() } != 0 {
        anyhow::bail!("Linux patch actions require the Talos worker to run as root");
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn detect_linux_patch_package_manager() -> Result<LinuxPatchPackageManager> {
    if linux_command_available("apt-get") && linux_command_available("apt") {
        return Ok(LinuxPatchPackageManager::Apt);
    }
    if linux_command_available("dnf") {
        return Ok(LinuxPatchPackageManager::Dnf);
    }
    if linux_command_available("yum") {
        return Ok(LinuxPatchPackageManager::Yum);
    }
    anyhow::bail!("no supported Linux package manager found (expected apt, dnf, or yum)");
}

#[cfg(target_os = "linux")]
fn ensure_linux_apt_prerequisites() -> Result<()> {
    ensure_linux_patch_root()?;
    if !linux_command_available("apt-get") {
        anyhow::bail!("apt-get is not available; only Debian/Ubuntu apt is supported");
    }
    if !linux_command_available("apt") {
        anyhow::bail!("apt is not available; only Debian/Ubuntu apt is supported");
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn apply_apt_environment(command: &mut std::process::Command) -> &mut std::process::Command {
    command
        .env("DEBIAN_FRONTEND", "noninteractive")
        .env("APT_LISTCHANGES_FRONTEND", "none")
}

#[cfg(target_os = "linux")]
fn run_apt_program_blocking(
    program: &str,
    args: &[String],
    description: &str,
) -> Result<AptCommandOutput> {
    run_linux_program_blocking(program, args, description, &[0])
}

#[cfg(target_os = "linux")]
fn run_linux_program_blocking(
    program: &str,
    args: &[String],
    description: &str,
    accepted_exit_codes: &[i32],
) -> Result<AptCommandOutput> {
    run_linux_program_blocking_with_timeout(
        program,
        args,
        description,
        accepted_exit_codes,
        linux_patch_command_timeout(),
    )
}

#[cfg(target_os = "linux")]
fn linux_command_output_temp_path(kind: &str) -> std::path::PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "talos_patch_{}_{}_{}_{}.log",
        kind,
        std::process::id(),
        nanos,
        Uuid::new_v4()
    ))
}

#[cfg(target_os = "linux")]
fn read_linux_command_output(path: &std::path::Path) -> String {
    let mut bytes = Vec::new();
    if let Ok(mut file) = File::open(path) {
        let _ = file.read_to_end(&mut bytes);
    }
    String::from_utf8_lossy(&bytes).to_string()
}

#[cfg(target_os = "linux")]
fn cleanup_linux_command_output(stdout_path: &std::path::Path, stderr_path: &std::path::Path) {
    let _ = std::fs::remove_file(stdout_path);
    let _ = std::fs::remove_file(stderr_path);
}

#[cfg(target_os = "linux")]
fn linux_command_failure_detail(stdout: &str, stderr: &str) -> String {
    [stdout.trim(), stderr.trim()]
        .into_iter()
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(target_os = "linux")]
fn run_linux_program_blocking_with_timeout(
    program: &str,
    args: &[String],
    description: &str,
    accepted_exit_codes: &[i32],
    timeout_duration: Duration,
) -> Result<AptCommandOutput> {
    let stdout_path = linux_command_output_temp_path("stdout");
    let stderr_path = linux_command_output_temp_path("stderr");
    let stdout_file = File::create(&stdout_path)
        .with_context(|| format!("create stdout capture for {description}"))?;
    let stderr_file = File::create(&stderr_path)
        .with_context(|| format!("create stderr capture for {description}"))?;
    let mut command = std::process::Command::new(program);
    if program == "apt" || program == "apt-get" {
        apply_apt_environment(&mut command);
    }
    let mut child = command
        .args(args)
        .stdout(Stdio::from(stdout_file))
        .stderr(Stdio::from(stderr_file))
        .spawn()
        .with_context(|| format!("launch {description}"))?;

    let started_at = Instant::now();
    let status = loop {
        if let Some(status) = child
            .try_wait()
            .with_context(|| format!("poll {description}"))?
        {
            break status;
        }
        if started_at.elapsed() >= timeout_duration {
            let _ = child.kill();
            let _ = child.wait();
            let stdout = read_linux_command_output(&stdout_path);
            let stderr = read_linux_command_output(&stderr_path);
            cleanup_linux_command_output(&stdout_path, &stderr_path);
            let detail = linux_command_failure_detail(&stdout, &stderr);
            anyhow::bail!(
                "{description} timed out after {} seconds{}{}",
                timeout_duration.as_secs(),
                if detail.is_empty() { "" } else { ": " },
                detail
            );
        }
        let remaining = timeout_duration.saturating_sub(started_at.elapsed());
        std::thread::sleep(std::cmp::min(Duration::from_millis(250), remaining));
    };

    let stdout = read_linux_command_output(&stdout_path);
    let stderr = read_linux_command_output(&stderr_path);
    cleanup_linux_command_output(&stdout_path, &stderr_path);

    let status_code = status.code();
    let accepted = status.success()
        || status_code
            .map(|code| accepted_exit_codes.contains(&code))
            .unwrap_or(false);
    if !accepted {
        let detail = linux_command_failure_detail(&stdout, &stderr);
        anyhow::bail!(
            "{description} failed with {}{}{}",
            status,
            if detail.is_empty() { "" } else { ": " },
            detail
        );
    }
    Ok(AptCommandOutput {
        status_code,
        stdout,
        stderr,
    })
}

#[cfg(target_os = "linux")]
fn refresh_linux_patch_metadata_blocking() -> Result<()> {
    match detect_linux_patch_package_manager()? {
        LinuxPatchPackageManager::Apt => refresh_apt_metadata_blocking(),
        manager @ (LinuxPatchPackageManager::Dnf | LinuxPatchPackageManager::Yum) => {
            refresh_dnf_yum_metadata_blocking(manager)
        }
    }
}

#[cfg(target_os = "linux")]
fn refresh_apt_metadata_blocking() -> Result<()> {
    ensure_linux_apt_prerequisites()?;
    run_apt_program_blocking("apt-get", &apt_get_update_args(), "apt-get update").map(|_| ())
}

#[cfg(target_os = "linux")]
fn query_apt_candidates_blocking() -> Result<Vec<AptPatchCandidate>> {
    ensure_linux_apt_prerequisites()?;
    let output = run_apt_program_blocking(
        "apt",
        &["list".to_string(), "--upgradable".to_string()],
        "apt list --upgradable",
    )?;
    Ok(parse_apt_upgradable_candidates(
        &output.stdout,
        linux_reboot_required_for_patch(),
    ))
}

#[cfg(target_os = "linux")]
fn ensure_linux_rpm_prerequisites(manager: LinuxPatchPackageManager) -> Result<()> {
    ensure_linux_patch_root()?;
    if !linux_command_available(manager.command()) {
        anyhow::bail!(
            "{} is not available; Fedora/RHEL patching requires dnf or yum",
            manager.command()
        );
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn run_dnf_yum_program_blocking(
    manager: LinuxPatchPackageManager,
    args: &[String],
    description: &str,
    accepted_exit_codes: &[i32],
) -> Result<AptCommandOutput> {
    run_linux_program_blocking(manager.command(), args, description, accepted_exit_codes)
}

#[cfg(target_os = "linux")]
fn refresh_dnf_yum_metadata_blocking(manager: LinuxPatchPackageManager) -> Result<()> {
    ensure_linux_rpm_prerequisites(manager)?;
    run_dnf_yum_program_blocking(
        manager,
        &dnf_yum_makecache_args(manager),
        &format!("{} makecache", manager.label()),
        &[0],
    )
    .map(|_| ())
}

#[cfg(target_os = "linux")]
fn query_rpm_installed_versions_blocking() -> HashMap<String, String> {
    let args = vec![
        "-qa".to_string(),
        "--qf".to_string(),
        "%{NAME}\t%{EVR}\n".to_string(),
    ];
    let Ok(output) =
        run_linux_program_blocking("rpm", &args, "rpm installed package version query", &[0])
    else {
        return HashMap::new();
    };
    output
        .stdout
        .lines()
        .filter_map(|line| {
            let (name, version) = line.split_once('\t')?;
            let name = name.trim();
            let version = version.trim();
            if name.is_empty() || version.is_empty() {
                return None;
            }
            Some((name.to_string(), version.to_string()))
        })
        .collect()
}

#[cfg(target_os = "linux")]
fn query_dnf_yum_candidates_blocking(
    manager: LinuxPatchPackageManager,
) -> Result<Vec<RpmPatchCandidate>> {
    ensure_linux_rpm_prerequisites(manager)?;
    let output = run_dnf_yum_program_blocking(
        manager,
        &dnf_yum_check_update_args(),
        &format!("{} check-update", manager.label()),
        &[0, 100],
    )?;
    let mut candidates =
        parse_dnf_yum_check_update_candidates(&output.stdout, linux_reboot_required_for_patch());
    let installed_versions = query_rpm_installed_versions_blocking();
    for candidate in &mut candidates {
        candidate.current_version = installed_versions.get(&candidate.package).cloned();
    }
    Ok(candidates)
}

#[cfg(target_os = "linux")]
fn linux_reboot_required_for_patch() -> bool {
    std::fs::metadata("/var/run/reboot-required").is_ok()
}

#[cfg(target_os = "linux")]
fn linux_rpm_reboot_required_for_patch(selected: &[RpmPatchCandidate]) -> bool {
    if linux_reboot_required_for_patch() {
        return true;
    }
    if selected.iter().any(|candidate| candidate.requires_reboot) {
        return true;
    }
    if linux_command_available("needs-restarting") {
        let args = vec!["-r".to_string()];
        if let Ok(output) =
            run_linux_program_blocking("needs-restarting", &args, "needs-restarting -r", &[0, 1])
        {
            return output.status_code == Some(1);
        }
    }
    false
}

#[cfg(target_os = "linux")]
fn apt_failure_outcome(
    job: &PatchRemediationJob,
    phase: &str,
    selected: &[AptPatchCandidate],
    evidence_updates: Vec<Value>,
    error: String,
    downloaded_count: usize,
) -> PatchExecutionOutcome {
    PatchExecutionOutcome {
        status: "failed",
        evidence: json!({
            "phase": phase,
            "job": job_summary(job),
            "updates": evidence_updates,
            "summary": apt_progress_summary(
                selected.len(),
                downloaded_count,
                0,
                selected.len(),
                0,
                linux_reboot_required_for_patch()
            ),
            "error": error
        }),
        force_reboot_after_report: false,
    }
}

#[cfg(target_os = "linux")]
fn execute_patch_job_linux_apt(
    job: &PatchRemediationJob,
    progress_reporter: Option<&PatchProgressReporter>,
) -> Result<PatchExecutionOutcome> {
    ensure_linux_apt_prerequisites()?;

    let requested = requested_update_keys(job);
    let behavior = reboot_behavior(job);
    let download_only = is_download_only_job(job);

    let searching_snapshot = PatchProgressSnapshot {
        phase: "searching",
        overall_percent: 0,
        phase_percent: 0,
        current_update_index: None,
        current_update_percent: None,
    };
    send_patch_job_progress(
        progress_reporter,
        job,
        "searching",
        &[],
        apt_progress_summary(0, 0, 0, 0, 0, false),
        &searching_snapshot,
    );
    let searching_heartbeat = start_patch_job_progress_heartbeat(
        progress_reporter,
        job,
        "searching",
        &[],
        apt_progress_summary(0, 0, 0, 0, 0, false),
        &searching_snapshot,
    );

    if let Err(error) = refresh_apt_metadata_blocking() {
        drop(searching_heartbeat);
        let message = format!("{error:#}");
        send_patch_job_progress_with_status(
            progress_reporter,
            job,
            "failed",
            "searching",
            &[],
            apt_progress_summary(0, 0, 0, 1, 0, false),
            &PatchProgressSnapshot {
                phase: "searching",
                overall_percent: 100,
                phase_percent: 100,
                current_update_index: None,
                current_update_percent: None,
            },
            Some(&message),
        );
        return Ok(failed_outcome(job, "searching", message, Vec::new()));
    }

    let candidates = match query_apt_candidates_blocking() {
        Ok(candidates) => candidates,
        Err(error) => {
            drop(searching_heartbeat);
            let message = format!("{error:#}");
            send_patch_job_progress_with_status(
                progress_reporter,
                job,
                "failed",
                "searching",
                &[],
                apt_progress_summary(0, 0, 0, 1, 0, false),
                &PatchProgressSnapshot {
                    phase: "searching",
                    overall_percent: 100,
                    phase_percent: 100,
                    current_update_index: None,
                    current_update_percent: None,
                },
                Some(&message),
            );
            return Ok(failed_outcome(job, "searching", message, Vec::new()));
        }
    };
    drop(searching_heartbeat);

    let mut selected = Vec::new();
    let mut evidence_updates = Vec::new();
    for candidate in candidates {
        let matched = apt_candidate_matches_requested(&candidate, &requested);
        if matched {
            selected.push(candidate.clone());
        }
        evidence_updates.push(apt_candidate_evidence(&candidate, matched));
    }

    for requested_key in &requested {
        if !selected.iter().any(|candidate| {
            apt_candidate_matches_requested(candidate, &HashSet::from([requested_key.clone()]))
        }) {
            evidence_updates.push(json!({
                "updateKey": requested_key,
                "title": Value::Null,
                "kbArticle": Value::Null,
                "matched": false,
                "downloaded": false,
                "installed": false,
                "resultCode": Value::Null,
                "result": "not_found",
                "hresult": Value::Null,
                "requiresReboot": false
            }));
        }
    }

    send_patch_job_progress(
        progress_reporter,
        job,
        "searching",
        &apt_update_progress_values(&selected, "queued"),
        apt_progress_summary(selected.len(), 0, 0, 0, 0, false),
        &PatchProgressSnapshot {
            phase: "searching",
            overall_percent: 0,
            phase_percent: 100,
            current_update_index: None,
            current_update_percent: None,
        },
    );

    if selected.is_empty() {
        let skipped = requested.len();
        send_patch_job_progress_with_status(
            progress_reporter,
            job,
            "completed",
            "finalizing",
            &[],
            apt_progress_summary(0, 0, 0, 0, skipped, linux_reboot_required_for_patch()),
            &PatchProgressSnapshot {
                phase: "finalizing",
                overall_percent: 100,
                phase_percent: 100,
                current_update_index: None,
                current_update_percent: None,
            },
            None,
        );
        return Ok(PatchExecutionOutcome {
            status: "completed",
            evidence: json!({
                "phase": "completed",
                "mode": if download_only { "download" } else { "install" },
                "job": job_summary(job),
                "updates": evidence_updates,
                "summary": apt_progress_summary(0, 0, 0, 0, skipped, linux_reboot_required_for_patch()),
                "error": Value::Null
            }),
            force_reboot_after_report: false,
        });
    }

    let package_specs = apt_package_specs(&selected);
    let downloading_updates = apt_update_progress_values(&selected, "downloading");
    let downloading_summary = apt_progress_summary(selected.len(), 0, 0, 0, 0, false);
    let downloading_snapshot = PatchProgressSnapshot {
        phase: "downloading",
        overall_percent: 0,
        phase_percent: 0,
        current_update_index: None,
        current_update_percent: None,
    };
    send_patch_job_progress(
        progress_reporter,
        job,
        "downloading",
        &downloading_updates,
        downloading_summary.clone(),
        &downloading_snapshot,
    );
    let download_heartbeat = start_patch_job_progress_heartbeat(
        progress_reporter,
        job,
        "downloading",
        &downloading_updates,
        downloading_summary,
        &downloading_snapshot,
    );
    let download_result = run_apt_program_blocking(
        "apt-get",
        &apt_get_download_args(&package_specs),
        "apt-get download-only install",
    );
    drop(download_heartbeat);
    let download_result = match download_result {
        Ok(result) => result,
        Err(error) => {
            let message = format!("{error:#}");
            mark_apt_evidence_updates(
                &mut evidence_updates,
                &selected,
                false,
                false,
                "failed",
                None,
                Some(&message),
            );
            send_patch_job_progress_with_status(
                progress_reporter,
                job,
                "failed",
                "downloading",
                &apt_update_progress_values(&selected, "failed"),
                apt_progress_summary(selected.len(), 0, 0, selected.len(), 0, false),
                &PatchProgressSnapshot {
                    phase: "downloading",
                    overall_percent: 100,
                    phase_percent: 100,
                    current_update_index: None,
                    current_update_percent: None,
                },
                Some(&message),
            );
            return Ok(apt_failure_outcome(
                job,
                "downloading",
                &selected,
                evidence_updates,
                message,
                0,
            ));
        }
    };

    mark_apt_evidence_updates(
        &mut evidence_updates,
        &selected,
        true,
        false,
        "downloaded",
        download_result.status_code,
        None,
    );

    if download_only {
        send_patch_job_progress_with_status(
            progress_reporter,
            job,
            "completed",
            "finalizing",
            &apt_terminal_progress_values(&selected, &evidence_updates),
            apt_progress_summary(selected.len(), selected.len(), 0, 0, 0, false),
            &PatchProgressSnapshot {
                phase: "finalizing",
                overall_percent: 100,
                phase_percent: 100,
                current_update_index: None,
                current_update_percent: None,
            },
            None,
        );
        return Ok(PatchExecutionOutcome {
            status: "completed",
            evidence: json!({
                "phase": "completed",
                "mode": "download",
                "job": job_summary(job),
                "updates": evidence_updates,
                "summary": {
                    "matched": selected.len(),
                    "downloaded": selected.len(),
                    "installed": 0,
                    "failed": 0,
                    "skipped": 0,
                    "rebootRequired": false,
                    "downloadResultCode": download_result.status_code,
                    "downloadResult": "succeeded",
                    "stdout": download_result.stdout,
                    "stderr": download_result.stderr
                },
                "error": Value::Null
            }),
            force_reboot_after_report: false,
        });
    }

    let installing_updates = apt_update_progress_values(&selected, "installing");
    let installing_summary = apt_progress_summary(selected.len(), selected.len(), 0, 0, 0, false);
    let installing_snapshot = PatchProgressSnapshot {
        phase: "installing",
        overall_percent: 50,
        phase_percent: 0,
        current_update_index: None,
        current_update_percent: None,
    };
    send_patch_job_progress(
        progress_reporter,
        job,
        "installing",
        &installing_updates,
        installing_summary.clone(),
        &installing_snapshot,
    );
    let install_heartbeat = start_patch_job_progress_heartbeat(
        progress_reporter,
        job,
        "installing",
        &installing_updates,
        installing_summary,
        &installing_snapshot,
    );
    let install_result = run_apt_program_blocking(
        "apt-get",
        &apt_get_install_args(&package_specs),
        "apt-get install",
    );
    drop(install_heartbeat);
    let install_result = match install_result {
        Ok(result) => result,
        Err(error) => {
            let message = format!("{error:#}");
            mark_apt_evidence_updates(
                &mut evidence_updates,
                &selected,
                true,
                false,
                "failed",
                None,
                Some(&message),
            );
            send_patch_job_progress_with_status(
                progress_reporter,
                job,
                "failed",
                "installing",
                &apt_terminal_progress_values(&selected, &evidence_updates),
                apt_progress_summary(selected.len(), selected.len(), 0, selected.len(), 0, false),
                &PatchProgressSnapshot {
                    phase: "installing",
                    overall_percent: 100,
                    phase_percent: 100,
                    current_update_index: None,
                    current_update_percent: None,
                },
                Some(&message),
            );
            return Ok(apt_failure_outcome(
                job,
                "installing",
                &selected,
                evidence_updates,
                message,
                selected.len(),
            ));
        }
    };

    let reboot_required = linux_reboot_required_for_patch();
    mark_apt_evidence_updates(
        &mut evidence_updates,
        &selected,
        true,
        true,
        "installed",
        install_result.status_code,
        None,
    );
    send_patch_job_progress_with_status(
        progress_reporter,
        job,
        "completed",
        "finalizing",
        &apt_terminal_progress_values(&selected, &evidence_updates),
        apt_progress_summary(
            selected.len(),
            selected.len(),
            selected.len(),
            0,
            0,
            reboot_required,
        ),
        &PatchProgressSnapshot {
            phase: "finalizing",
            overall_percent: 100,
            phase_percent: 100,
            current_update_index: None,
            current_update_percent: None,
        },
        None,
    );
    let force_reboot_after_report = reboot_required && behavior == "force";

    Ok(PatchExecutionOutcome {
        status: "completed",
        evidence: json!({
            "phase": "completed",
            "mode": "install",
            "job": job_summary(job),
            "updates": evidence_updates,
            "summary": {
                "matched": selected.len(),
                "downloaded": selected.len(),
                "installed": selected.len(),
                "failed": 0,
                "skipped": 0,
                "rebootRequired": reboot_required,
                "downloadResultCode": download_result.status_code,
                "downloadResult": "succeeded",
                "installResultCode": install_result.status_code,
                "installResult": "succeeded",
                "rebootBehavior": behavior,
                "downloadStdout": download_result.stdout,
                "downloadStderr": download_result.stderr,
                "installStdout": install_result.stdout,
                "installStderr": install_result.stderr
            },
            "error": Value::Null
        }),
        force_reboot_after_report,
    })
}

#[cfg(target_os = "linux")]
fn rpm_transaction_failure_outcome(
    job: &PatchRemediationJob,
    phase: &str,
    selected: &[RpmPatchCandidate],
    package_specs: &[String],
    evidence_updates: Vec<Value>,
    error: String,
    downloaded_count: usize,
) -> PatchExecutionOutcome {
    PatchExecutionOutcome {
        status: "failed",
        evidence: json!({
            "phase": phase,
            "job": job_summary(job),
            "updates": evidence_updates,
            "failureScope": "transaction",
            "transactionPackageSpecs": package_specs,
            "summary": {
                "matched": selected.len(),
                "downloaded": downloaded_count,
                "installed": 0,
                "failed": 0,
                "skipped": 0,
                "rebootRequired": linux_rpm_reboot_required_for_patch(selected),
                "transactionFailure": true
            },
            "error": error
        }),
        force_reboot_after_report: false,
    }
}

#[cfg(target_os = "linux")]
fn execute_patch_job_linux_rpm(
    job: &PatchRemediationJob,
    progress_reporter: Option<&PatchProgressReporter>,
    manager: LinuxPatchPackageManager,
) -> Result<PatchExecutionOutcome> {
    ensure_linux_rpm_prerequisites(manager)?;

    let requested_update_keys = requested_update_key_list(job);
    let requested = requested_update_keys
        .iter()
        .cloned()
        .collect::<HashSet<_>>();
    let behavior = reboot_behavior(job);
    let download_only = is_download_only_job(job);

    let searching_snapshot = PatchProgressSnapshot {
        phase: "searching",
        overall_percent: 0,
        phase_percent: 0,
        current_update_index: None,
        current_update_percent: None,
    };
    send_patch_job_progress(
        progress_reporter,
        job,
        "searching",
        &[],
        apt_progress_summary(0, 0, 0, 0, 0, false),
        &searching_snapshot,
    );
    let searching_heartbeat = start_patch_job_progress_heartbeat(
        progress_reporter,
        job,
        "searching",
        &[],
        apt_progress_summary(0, 0, 0, 0, 0, false),
        &searching_snapshot,
    );

    if let Err(error) = refresh_dnf_yum_metadata_blocking(manager) {
        drop(searching_heartbeat);
        let message = format!("{error:#}");
        send_patch_job_progress_with_status(
            progress_reporter,
            job,
            "failed",
            "searching",
            &[],
            apt_progress_summary(0, 0, 0, 1, 0, false),
            &PatchProgressSnapshot {
                phase: "searching",
                overall_percent: 100,
                phase_percent: 100,
                current_update_index: None,
                current_update_percent: None,
            },
            Some(&message),
        );
        return Ok(failed_outcome(job, "searching", message, Vec::new()));
    }

    let candidates = match query_dnf_yum_candidates_blocking(manager) {
        Ok(candidates) => candidates,
        Err(error) => {
            drop(searching_heartbeat);
            let message = format!("{error:#}");
            send_patch_job_progress_with_status(
                progress_reporter,
                job,
                "failed",
                "searching",
                &[],
                apt_progress_summary(0, 0, 0, 1, 0, false),
                &PatchProgressSnapshot {
                    phase: "searching",
                    overall_percent: 100,
                    phase_percent: 100,
                    current_update_index: None,
                    current_update_percent: None,
                },
                Some(&message),
            );
            return Ok(failed_outcome(job, "searching", message, Vec::new()));
        }
    };
    drop(searching_heartbeat);

    let mut selected = Vec::new();
    let mut evidence_updates = Vec::new();
    for candidate in candidates {
        let matched = rpm_candidate_matches_requested(&candidate, &requested);
        if matched {
            selected.push(candidate.clone());
        }
        evidence_updates.push(rpm_candidate_evidence(&candidate, matched));
    }

    for requested_key in &requested {
        if !selected.iter().any(|candidate| {
            rpm_candidate_matches_requested(candidate, &HashSet::from([requested_key.clone()]))
        }) {
            evidence_updates.push(json!({
                "updateKey": requested_key,
                "title": Value::Null,
                "kbArticle": Value::Null,
                "matched": false,
                "downloaded": false,
                "installed": false,
                "resultCode": Value::Null,
                "result": "not_found",
                "hresult": Value::Null,
                "requiresReboot": false
            }));
        }
    }

    send_patch_job_progress(
        progress_reporter,
        job,
        "searching",
        &rpm_update_progress_values(&selected, "queued"),
        apt_progress_summary(selected.len(), 0, 0, 0, 0, false),
        &PatchProgressSnapshot {
            phase: "searching",
            overall_percent: 0,
            phase_percent: 100,
            current_update_index: None,
            current_update_percent: None,
        },
    );

    if selected.is_empty() {
        let skipped = requested.len();
        send_patch_job_progress_with_status(
            progress_reporter,
            job,
            "completed",
            "finalizing",
            &[],
            apt_progress_summary(
                0,
                0,
                0,
                0,
                skipped,
                linux_rpm_reboot_required_for_patch(&[]),
            ),
            &PatchProgressSnapshot {
                phase: "finalizing",
                overall_percent: 100,
                phase_percent: 100,
                current_update_index: None,
                current_update_percent: None,
            },
            None,
        );
        return Ok(PatchExecutionOutcome {
            status: "completed",
            evidence: json!({
                "phase": "completed",
                "mode": if download_only { "download" } else { "install" },
                "packageManager": manager.label(),
                "job": job_summary(job),
                "updates": evidence_updates,
                "summary": apt_progress_summary(0, 0, 0, 0, skipped, linux_rpm_reboot_required_for_patch(&[])),
                "error": Value::Null
            }),
            force_reboot_after_report: false,
        });
    }

    let download_package_specs = rpm_package_specs(&selected);
    let install_package_specs = rpm_install_package_specs(&selected, &requested_update_keys);
    let downloading_updates = rpm_update_progress_values(&selected, "downloading");
    let downloading_summary = apt_progress_summary(selected.len(), 0, 0, 0, 0, false);
    let downloading_snapshot = PatchProgressSnapshot {
        phase: "downloading",
        overall_percent: 0,
        phase_percent: 0,
        current_update_index: None,
        current_update_percent: None,
    };
    send_patch_job_progress(
        progress_reporter,
        job,
        "downloading",
        &downloading_updates,
        downloading_summary.clone(),
        &downloading_snapshot,
    );
    let download_heartbeat = start_patch_job_progress_heartbeat(
        progress_reporter,
        job,
        "downloading",
        &downloading_updates,
        downloading_summary,
        &downloading_snapshot,
    );
    let download_result = run_dnf_yum_program_blocking(
        manager,
        &dnf_yum_download_args(if download_only {
            &download_package_specs
        } else {
            &install_package_specs
        }),
        &format!("{} download-only upgrade", manager.label()),
        &[0],
    );
    drop(download_heartbeat);
    let download_result = match download_result {
        Ok(result) => result,
        Err(error) => {
            let message = format!("{error:#}");
            send_patch_job_progress_with_status(
                progress_reporter,
                job,
                "failed",
                "downloading",
                &rpm_update_progress_values(&selected, "queued"),
                apt_progress_summary(selected.len(), 0, 0, 0, 0, false),
                &PatchProgressSnapshot {
                    phase: "downloading",
                    overall_percent: 100,
                    phase_percent: 100,
                    current_update_index: None,
                    current_update_percent: None,
                },
                Some(&message),
            );
            return Ok(rpm_transaction_failure_outcome(
                job,
                "downloading",
                &selected,
                if download_only {
                    &download_package_specs
                } else {
                    &install_package_specs
                },
                evidence_updates,
                message,
                0,
            ));
        }
    };

    mark_rpm_evidence_updates(
        &mut evidence_updates,
        &selected,
        true,
        false,
        "downloaded",
        download_result.status_code,
        None,
    );

    if download_only {
        let reboot_required = linux_rpm_reboot_required_for_patch(&selected);
        send_patch_job_progress_with_status(
            progress_reporter,
            job,
            "completed",
            "finalizing",
            &rpm_terminal_progress_values(&selected, &evidence_updates),
            apt_progress_summary(selected.len(), selected.len(), 0, 0, 0, reboot_required),
            &PatchProgressSnapshot {
                phase: "finalizing",
                overall_percent: 100,
                phase_percent: 100,
                current_update_index: None,
                current_update_percent: None,
            },
            None,
        );
        return Ok(PatchExecutionOutcome {
            status: "completed",
            evidence: json!({
                "phase": "completed",
                "mode": "download",
                "packageManager": manager.label(),
                "job": job_summary(job),
                "updates": evidence_updates,
                "summary": {
                    "matched": selected.len(),
                    "downloaded": selected.len(),
                    "installed": 0,
                    "failed": 0,
                    "skipped": 0,
                    "rebootRequired": reboot_required,
                    "transactionPackageSpecs": download_package_specs,
                    "downloadResultCode": download_result.status_code,
                    "downloadResult": "succeeded",
                    "stdout": download_result.stdout,
                    "stderr": download_result.stderr
                },
                "error": Value::Null
            }),
            force_reboot_after_report: false,
        });
    }

    let installing_updates = rpm_update_progress_values(&selected, "installing");
    let installing_summary = apt_progress_summary(selected.len(), selected.len(), 0, 0, 0, false);
    let installing_snapshot = PatchProgressSnapshot {
        phase: "installing",
        overall_percent: 50,
        phase_percent: 0,
        current_update_index: None,
        current_update_percent: None,
    };
    send_patch_job_progress(
        progress_reporter,
        job,
        "installing",
        &installing_updates,
        installing_summary.clone(),
        &installing_snapshot,
    );
    let install_heartbeat = start_patch_job_progress_heartbeat(
        progress_reporter,
        job,
        "installing",
        &installing_updates,
        installing_summary,
        &installing_snapshot,
    );
    let install_result = run_dnf_yum_program_blocking(
        manager,
        &dnf_yum_install_args(&install_package_specs),
        &format!("{} upgrade", manager.label()),
        &[0],
    );
    drop(install_heartbeat);
    let install_result = match install_result {
        Ok(result) => result,
        Err(error) => {
            let message = format!("{error:#}");
            send_patch_job_progress_with_status(
                progress_reporter,
                job,
                "failed",
                "installing",
                &rpm_terminal_progress_values(&selected, &evidence_updates),
                apt_progress_summary(selected.len(), selected.len(), 0, 0, 0, false),
                &PatchProgressSnapshot {
                    phase: "installing",
                    overall_percent: 100,
                    phase_percent: 100,
                    current_update_index: None,
                    current_update_percent: None,
                },
                Some(&message),
            );
            return Ok(rpm_transaction_failure_outcome(
                job,
                "installing",
                &selected,
                &install_package_specs,
                evidence_updates,
                message,
                selected.len(),
            ));
        }
    };

    let reboot_required = linux_rpm_reboot_required_for_patch(&selected);
    mark_rpm_evidence_updates(
        &mut evidence_updates,
        &selected,
        true,
        true,
        "installed",
        install_result.status_code,
        None,
    );
    send_patch_job_progress_with_status(
        progress_reporter,
        job,
        "completed",
        "finalizing",
        &rpm_terminal_progress_values(&selected, &evidence_updates),
        apt_progress_summary(
            selected.len(),
            selected.len(),
            selected.len(),
            0,
            0,
            reboot_required,
        ),
        &PatchProgressSnapshot {
            phase: "finalizing",
            overall_percent: 100,
            phase_percent: 100,
            current_update_index: None,
            current_update_percent: None,
        },
        None,
    );
    let force_reboot_after_report = reboot_required && behavior == "force";

    Ok(PatchExecutionOutcome {
        status: "completed",
        evidence: json!({
            "phase": "completed",
            "mode": "install",
            "packageManager": manager.label(),
            "job": job_summary(job),
            "updates": evidence_updates,
            "summary": {
                "matched": selected.len(),
                "downloaded": selected.len(),
                "installed": selected.len(),
                "failed": 0,
                "skipped": 0,
                "rebootRequired": reboot_required,
                "transactionPackageSpecs": install_package_specs,
                "downloadResultCode": download_result.status_code,
                "downloadResult": "succeeded",
                "installResultCode": install_result.status_code,
                "installResult": "succeeded",
                "rebootBehavior": behavior,
                "downloadStdout": download_result.stdout,
                "downloadStderr": download_result.stderr,
                "installStdout": install_result.stdout,
                "installStderr": install_result.stderr
            },
            "error": Value::Null
        }),
        force_reboot_after_report,
    })
}

pub(crate) fn schedule_forced_reboot_after_patch() -> Result<Value> {
    #[cfg(target_os = "windows")]
    {
        return schedule_update_reboot_notice_flow();
    }

    #[cfg(target_os = "linux")]
    {
        let shutdown = std::process::Command::new("shutdown")
            .args(["-r", "+1", PATCH_REBOOT_MESSAGE])
            .status();
        match shutdown {
            Ok(status) if status.success() => {
                return Ok(json!({
                    "rebootNoticeShown": false,
                    "legacyScheduledReboot": true
                }));
            }
            Ok(status) => {
                warn!(%status, "linux shutdown reboot scheduling failed; trying systemctl")
            }
            Err(error) => warn!(%error, "linux shutdown command failed; trying systemctl"),
        }

        let status = std::process::Command::new("systemctl")
            .arg("reboot")
            .status()
            .context("launch systemctl reboot")?;
        if !status.success() {
            anyhow::bail!("systemctl reboot exited with {status}");
        }
        return Ok(json!({
            "rebootNoticeShown": false,
            "legacyScheduledReboot": false
        }));
    }

    #[cfg(target_os = "macos")]
    {
        return schedule_update_reboot_notice_flow();
    }

    #[cfg(all(
        not(target_os = "windows"),
        not(target_os = "linux"),
        not(target_os = "macos")
    ))]
    {
        anyhow::bail!("forced reboot scheduling is only supported on Windows, macOS, and Linux");
    }
}

#[cfg(any(test, target_os = "windows"))]
fn windows_immediate_reboot_command() -> (&'static str, Vec<&'static str>) {
    (
        "shutdown.exe",
        vec!["/r", "/t", "0", "/c", PATCH_REBOOT_MESSAGE],
    )
}

#[cfg(any(test, target_os = "macos"))]
fn macos_immediate_reboot_command() -> (&'static str, Vec<&'static str>) {
    ("/sbin/shutdown", vec!["-r", "now", PATCH_REBOOT_MESSAGE])
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
fn execute_immediate_patch_reboot() -> Result<()> {
    #[cfg(target_os = "windows")]
    let (program, args) = windows_immediate_reboot_command();
    #[cfg(target_os = "macos")]
    let (program, args) = macos_immediate_reboot_command();

    let status = std::process::Command::new(program)
        .args(args)
        .status()
        .with_context(|| format!("launch immediate reboot command {program}"))?;
    if !status.success() {
        anyhow::bail!("{program} exited with {status}");
    }
    Ok(())
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct RebootNoticeStateFile {
    notice_id: String,
    deferrals_used: u32,
    max_deferrals: u32,
    #[serde(default)]
    next_notice_unix_ms: Option<u64>,
    #[serde(default)]
    deadline_unix_ms: Option<u64>,
    updated_at_unix_ms: u64,
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
#[derive(Debug, Clone, PartialEq, Eq)]
struct ActiveRebootNoticeFlow {
    notice_id: String,
    next_notice_unix_ms: u64,
    deadline_unix_ms: u64,
    deferrals_used: u32,
    max_deferrals: u32,
    delay_minutes: u32,
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RebootNoticeOutcome {
    Defer,
    RebootNow,
    Timeout,
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
fn active_reboot_notice_flow() -> &'static std::sync::Mutex<Option<ActiveRebootNoticeFlow>> {
    static ACTIVE: std::sync::OnceLock<std::sync::Mutex<Option<ActiveRebootNoticeFlow>>> =
        std::sync::OnceLock::new();
    ACTIVE.get_or_init(|| std::sync::Mutex::new(None))
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
fn reboot_notice_now_unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
fn unix_ms_after(duration: Duration) -> u64 {
    reboot_notice_now_unix_ms().saturating_add(duration.as_millis() as u64)
}

#[cfg(target_os = "windows")]
fn reboot_notice_state_path() -> std::path::PathBuf {
    std::env::var("PROGRAMDATA")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::path::PathBuf::from(r"C:\ProgramData"))
        .join("Talos")
        .join("patch_reboot_notice_state.json")
}

#[cfg(target_os = "macos")]
fn reboot_notice_state_path() -> std::path::PathBuf {
    std::path::PathBuf::from("/Library/Talos/patch_reboot_notice_state.json")
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
fn load_reboot_notice_state() -> Option<RebootNoticeStateFile> {
    let path = reboot_notice_state_path();
    let bytes = std::fs::read(&path).ok()?;
    serde_json::from_slice::<RebootNoticeStateFile>(&bytes)
        .map_err(|error| {
            warn!(
                path = %path.display(),
                %error,
                "failed to parse reboot notice state file"
            );
            error
        })
        .ok()
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
fn persist_reboot_notice_state(flow: &ActiveRebootNoticeFlow) -> Result<()> {
    let path = reboot_notice_state_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    let state = RebootNoticeStateFile {
        notice_id: flow.notice_id.clone(),
        deferrals_used: flow.deferrals_used,
        max_deferrals: flow.max_deferrals,
        next_notice_unix_ms: Some(flow.next_notice_unix_ms),
        deadline_unix_ms: Some(flow.deadline_unix_ms),
        updated_at_unix_ms: reboot_notice_now_unix_ms(),
    };
    let body = serde_json::to_vec_pretty(&state).context("serialize reboot notice state")?;
    std::fs::write(&path, body).with_context(|| format!("write {}", path.display()))?;
    Ok(())
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
fn clear_reboot_notice_state_file() -> Result<()> {
    let path = reboot_notice_state_path();
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(anyhow::anyhow!(error).context(format!("remove {}", path.display()))),
    }
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
fn set_active_reboot_notice_flow(flow: ActiveRebootNoticeFlow) {
    let Ok(mut guard) = active_reboot_notice_flow().lock() else {
        return;
    };
    *guard = Some(flow);
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
fn clear_active_reboot_notice_flow(notice_id: &str) {
    let Ok(mut guard) = active_reboot_notice_flow().lock() else {
        return;
    };
    if guard
        .as_ref()
        .map(|flow| flow.notice_id.as_str() == notice_id)
        .unwrap_or(false)
    {
        *guard = None;
    }
}

#[cfg(any(test, target_os = "windows", target_os = "macos"))]
fn reboot_notice_can_defer(deferrals_used: u32, max_deferrals: u32) -> bool {
    deferrals_used < max_deferrals
}

#[cfg(any(test, target_os = "windows", target_os = "macos"))]
fn next_reboot_notice_deferral_count(deferrals_used: u32, max_deferrals: u32) -> Option<u32> {
    reboot_notice_can_defer(deferrals_used, max_deferrals).then_some(deferrals_used + 1)
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
fn reboot_notice_evidence(flow: &ActiveRebootNoticeFlow) -> Value {
    json!({
        "rebootNoticeShown": true,
        "noticeId": flow.notice_id.clone(),
        "deadlineUnixMs": flow.deadline_unix_ms,
        "deferralsUsed": flow.deferrals_used,
        "maxDeferrals": flow.max_deferrals
    })
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
fn schedule_update_reboot_notice_flow() -> Result<Value> {
    let mut guard = active_reboot_notice_flow()
        .lock()
        .map_err(|_| anyhow::anyhow!("reboot notice flow lock poisoned"))?;
    if let Some(active) = guard.as_ref() {
        return Ok(reboot_notice_evidence(active));
    }

    let persisted = load_reboot_notice_state();
    let deferrals_used = persisted
        .as_ref()
        .map(|state| {
            state
                .deferrals_used
                .min(state.max_deferrals)
                .min(UPDATE_REBOOT_NOTICE_MAX_DEFERRALS)
        })
        .unwrap_or(0);
    let notice_id = persisted
        .as_ref()
        .map(|state| state.notice_id.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| Uuid::new_v4().to_string());
    let now = reboot_notice_now_unix_ms();
    let next_notice_unix_ms = persisted
        .as_ref()
        .and_then(|state| state.next_notice_unix_ms)
        .unwrap_or(now);
    let deadline_unix_ms = persisted
        .as_ref()
        .and_then(|state| state.deadline_unix_ms)
        .unwrap_or_else(|| {
            next_notice_unix_ms
                .max(now)
                .saturating_add(UPDATE_REBOOT_NOTICE_WARNING.as_millis() as u64)
        });
    let flow = ActiveRebootNoticeFlow {
        notice_id,
        next_notice_unix_ms,
        deadline_unix_ms,
        deferrals_used,
        max_deferrals: UPDATE_REBOOT_NOTICE_MAX_DEFERRALS,
        delay_minutes: (UPDATE_REBOOT_NOTICE_DELAY.as_secs() / 60) as u32,
    };
    if let Err(error) = persist_reboot_notice_state(&flow) {
        warn!(%error, "failed to persist reboot notice state before scheduling");
    }

    let thread_flow = flow.clone();
    std::thread::Builder::new()
        .name("talos-update-reboot-notice".to_string())
        .spawn(move || run_update_reboot_notice_flow(thread_flow))
        .context("spawn update reboot notice flow")?;

    *guard = Some(flow.clone());
    Ok(reboot_notice_evidence(&flow))
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
fn run_update_reboot_notice_flow(mut flow: ActiveRebootNoticeFlow) {
    loop {
        set_active_reboot_notice_flow(flow.clone());
        if let Err(error) = persist_reboot_notice_state(&flow) {
            warn!(%error, "failed to persist reboot notice state");
        }

        if flow.next_notice_unix_ms > reboot_notice_now_unix_ms() {
            let sleep_ms = flow
                .next_notice_unix_ms
                .saturating_sub(reboot_notice_now_unix_ms());
            if sleep_ms > 0 {
                std::thread::sleep(Duration::from_millis(sleep_ms));
            }
        }

        match present_update_reboot_notice_once(&flow) {
            RebootNoticeOutcome::Defer => {
                let Some(next_deferrals) =
                    next_reboot_notice_deferral_count(flow.deferrals_used, flow.max_deferrals)
                else {
                    info!(
                        notice_id = %flow.notice_id,
                        deferrals_used = flow.deferrals_used,
                        max_deferrals = flow.max_deferrals,
                        "reboot notice deferral limit reached; rebooting now"
                    );
                    break;
                };
                flow.deferrals_used = next_deferrals;
                flow.next_notice_unix_ms = unix_ms_after(UPDATE_REBOOT_NOTICE_DELAY);
                flow.deadline_unix_ms = flow
                    .next_notice_unix_ms
                    .saturating_add(UPDATE_REBOOT_NOTICE_WARNING.as_millis() as u64);
                set_active_reboot_notice_flow(flow.clone());
                if let Err(error) = persist_reboot_notice_state(&flow) {
                    warn!(%error, "failed to persist reboot notice deferral");
                }
                info!(
                    notice_id = %flow.notice_id,
                    deferrals_used = flow.deferrals_used,
                    max_deferrals = flow.max_deferrals,
                    delay_secs = UPDATE_REBOOT_NOTICE_DELAY.as_secs(),
                    "user deferred update reboot"
                );
                std::thread::sleep(UPDATE_REBOOT_NOTICE_DELAY);
            }
            RebootNoticeOutcome::RebootNow => {
                info!(notice_id = %flow.notice_id, "user requested immediate update reboot");
                break;
            }
            RebootNoticeOutcome::Timeout => {
                info!(notice_id = %flow.notice_id, "update reboot notice deadline reached");
                break;
            }
        }
    }

    if let Err(error) = clear_reboot_notice_state_file() {
        warn!(%error, "failed to clear reboot notice state before reboot command");
    }
    if let Err(error) = execute_immediate_patch_reboot() {
        warn!(%error, "immediate update reboot command failed");
    }
    clear_active_reboot_notice_flow(&flow.notice_id);
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
fn sleep_until_reboot_notice_deadline(deadline_unix_ms: u64) {
    let now = reboot_notice_now_unix_ms();
    if deadline_unix_ms > now {
        std::thread::sleep(Duration::from_millis(deadline_unix_ms - now));
    }
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
fn present_update_reboot_notice_once(flow: &ActiveRebootNoticeFlow) -> RebootNoticeOutcome {
    let listener = match std::net::TcpListener::bind("127.0.0.1:0") {
        Ok(listener) => listener,
        Err(error) => {
            warn!(%error, "failed to bind reboot notice localhost bridge");
            sleep_until_reboot_notice_deadline(flow.deadline_unix_ms);
            return RebootNoticeOutcome::Timeout;
        }
    };
    if let Err(error) = listener.set_nonblocking(true) {
        warn!(%error, "failed to set reboot notice listener nonblocking");
    }
    let port = match listener.local_addr() {
        Ok(addr) => addr.port(),
        Err(error) => {
            warn!(%error, "failed to read reboot notice listener address");
            sleep_until_reboot_notice_deadline(flow.deadline_unix_ms);
            return RebootNoticeOutcome::Timeout;
        }
    };
    let bridge_secret = Uuid::new_v4().to_string();
    let config = crate::chat::chat_launch::RebootNoticeLaunchConfig {
        notice_id: flow.notice_id.clone(),
        deadline_unix_ms: flow.deadline_unix_ms,
        deferrals_used: flow.deferrals_used,
        max_deferrals: flow.max_deferrals,
        delay_minutes: flow.delay_minutes,
    };
    let exe = crate::chat::chat_launch::worker_chat_exe_path();
    if let Err(error) = crate::chat::chat_launch::launch_update_reboot_notice_ui(
        0,
        &exe,
        port,
        &bridge_secret,
        &config,
    ) {
        warn!(
            %error,
            notice_id = %flow.notice_id,
            "failed to launch reboot notice UI; continuing countdown without deferral"
        );
        sleep_until_reboot_notice_deadline(flow.deadline_unix_ms);
        return RebootNoticeOutcome::Timeout;
    }

    let Some(mut stream) = accept_reboot_notice_stream(&listener, flow.deadline_unix_ms) else {
        warn!(
            notice_id = %flow.notice_id,
            "reboot notice UI did not connect; continuing countdown without deferral"
        );
        sleep_until_reboot_notice_deadline(flow.deadline_unix_ms);
        return RebootNoticeOutcome::Timeout;
    };

    let auth = match read_sync_chat_frame(&mut stream) {
        Ok(Some(frame)) => frame,
        Ok(None) => {
            sleep_until_reboot_notice_deadline(flow.deadline_unix_ms);
            return RebootNoticeOutcome::Timeout;
        }
        Err(error) => {
            warn!(%error, "failed to read reboot notice UI auth");
            sleep_until_reboot_notice_deadline(flow.deadline_unix_ms);
            return RebootNoticeOutcome::Timeout;
        }
    };
    if auth.0 != CHAT_MSG_AUTH || String::from_utf8_lossy(&auth.1).trim() != bridge_secret {
        warn!(
            frame_type = auth.0,
            body_len = auth.1.len(),
            "reboot notice UI auth mismatch"
        );
        sleep_until_reboot_notice_deadline(flow.deadline_unix_ms);
        return RebootNoticeOutcome::Timeout;
    }

    loop {
        let now = reboot_notice_now_unix_ms();
        if now >= flow.deadline_unix_ms {
            return RebootNoticeOutcome::Timeout;
        }
        let read_timeout = Duration::from_millis(flow.deadline_unix_ms - now);
        if let Err(error) = stream.set_read_timeout(Some(read_timeout)) {
            warn!(%error, "failed to set reboot notice read timeout");
        }
        let frame = match read_sync_chat_frame(&mut stream) {
            Ok(Some(frame)) => frame,
            Ok(None) => {
                sleep_until_reboot_notice_deadline(flow.deadline_unix_ms);
                return RebootNoticeOutcome::Timeout;
            }
            Err(error) => {
                warn!(%error, "failed to read reboot notice UI frame");
                sleep_until_reboot_notice_deadline(flow.deadline_unix_ms);
                return RebootNoticeOutcome::Timeout;
            }
        };
        if frame.0 != CHAT_MSG_CONTROL {
            continue;
        }
        let payload = match serde_json::from_slice::<WorkerChatControlPayload>(&frame.1) {
            Ok(payload) => payload,
            Err(error) => {
                warn!(%error, "failed to parse reboot notice control payload");
                continue;
            }
        };
        match payload {
            WorkerChatControlPayload::RebootNoticeReady { .. } => {}
            WorkerChatControlPayload::RebootNoticeAction { notice_id, action } => {
                if notice_id != flow.notice_id {
                    warn!(
                        expected_notice_id = %flow.notice_id,
                        actual_notice_id = %notice_id,
                        "ignoring reboot notice action for different notice"
                    );
                    continue;
                }
                return match action {
                    RebootNoticeAction::Defer => RebootNoticeOutcome::Defer,
                    RebootNoticeAction::RebootNow => RebootNoticeOutcome::RebootNow,
                };
            }
            WorkerChatControlPayload::AiRunnerApprovalRequest { .. }
            | WorkerChatControlPayload::AiRunnerApprovalDecision { .. } => {}
        }
    }
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
fn accept_reboot_notice_stream(
    listener: &std::net::TcpListener,
    deadline_unix_ms: u64,
) -> Option<std::net::TcpStream> {
    let accept_until = deadline_unix_ms.min(unix_ms_after(UPDATE_REBOOT_NOTICE_CONNECT_TIMEOUT));
    while reboot_notice_now_unix_ms() < accept_until {
        match listener.accept() {
            Ok((stream, _)) => return Some(stream),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(200));
            }
            Err(error) => {
                warn!(%error, "failed to accept reboot notice UI connection");
                return None;
            }
        }
    }
    None
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
fn read_sync_chat_frame(stream: &mut std::net::TcpStream) -> Result<Option<(u8, Vec<u8>)>> {
    use std::io::Read as _;

    let mut hdr = [0u8; 3];
    if let Err(error) = stream.read_exact(&mut hdr) {
        return match error.kind() {
            std::io::ErrorKind::UnexpectedEof
            | std::io::ErrorKind::TimedOut
            | std::io::ErrorKind::WouldBlock => Ok(None),
            _ => Err(anyhow::anyhow!("tcp reboot notice header: {error}")),
        };
    }
    let len = u16::from_be_bytes([hdr[1], hdr[2]]) as usize;
    if len > talos_protocol::CHAT_MAX_PAYLOAD_LEN {
        anyhow::bail!("tcp reboot notice payload too large");
    }
    let mut body = vec![0u8; len];
    if len > 0 {
        if let Err(error) = stream.read_exact(&mut body) {
            return match error.kind() {
                std::io::ErrorKind::UnexpectedEof
                | std::io::ErrorKind::TimedOut
                | std::io::ErrorKind::WouldBlock => Ok(None),
                _ => Err(anyhow::anyhow!("tcp reboot notice payload: {error}")),
            };
        }
    }
    Ok(Some((hdr[0], body)))
}

fn current_native_windows_update_control_state() -> Result<Value> {
    #[cfg(target_os = "windows")]
    {
        let disable_ux = read_registry_dword(
            r"HKLM\SOFTWARE\Policies\Microsoft\Windows\WindowsUpdate",
            "SetDisableUXWUAccess",
        )?;
        let no_auto_update = read_registry_dword(
            r"HKLM\SOFTWARE\Policies\Microsoft\Windows\WindowsUpdate\AU",
            "NoAutoUpdate",
        )?;
        return Ok(json!({
            "setDisableUxWuAccess": disable_ux,
            "noAutoUpdate": no_auto_update
        }));
    }

    #[cfg(not(target_os = "windows"))]
    {
        Ok(json!({ "supported": false }))
    }
}

async fn apply_native_windows_update_control(enabled: bool) -> Result<()> {
    tokio::task::spawn_blocking(move || apply_native_windows_update_control_blocking(enabled))
        .await
        .context("native Windows Update control task failed")?
}

fn apply_native_windows_update_control_blocking(enabled: bool) -> Result<()> {
    #[cfg(target_os = "windows")]
    {
        backup_native_windows_update_policy()?;
        if enabled {
            set_registry_dword(
                r"HKLM\SOFTWARE\Policies\Microsoft\Windows\WindowsUpdate",
                "SetDisableUXWUAccess",
                1,
            )?;
            set_registry_dword(
                r"HKLM\SOFTWARE\Policies\Microsoft\Windows\WindowsUpdate\AU",
                "NoAutoUpdate",
                1,
            )?;
        } else {
            restore_native_windows_update_policy()?;
        }
        return Ok(());
    }

    #[cfg(not(target_os = "windows"))]
    {
        let _ = enabled;
        Ok(())
    }
}

#[cfg(target_os = "windows")]
fn talos_patch_policy_backup_path() -> std::path::PathBuf {
    std::env::var_os("ProgramData")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from(r"C:\ProgramData"))
        .join("Talos")
        .join("patch_wu_policy_backup.json")
}

#[cfg(target_os = "windows")]
fn backup_native_windows_update_policy() -> Result<()> {
    let path = talos_patch_policy_backup_path();
    if path.exists() {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    let backup = json!({
        "createdAt": Utc::now().to_rfc3339(),
        "values": {
            "setDisableUxWuAccess": read_registry_dword(
                r"HKLM\SOFTWARE\Policies\Microsoft\Windows\WindowsUpdate",
                "SetDisableUXWUAccess",
            )?,
            "noAutoUpdate": read_registry_dword(
                r"HKLM\SOFTWARE\Policies\Microsoft\Windows\WindowsUpdate\AU",
                "NoAutoUpdate",
            )?
        }
    });
    std::fs::write(&path, serde_json::to_vec_pretty(&backup)?)
        .with_context(|| format!("write {}", path.display()))?;
    Ok(())
}

#[cfg(target_os = "windows")]
fn restore_native_windows_update_policy() -> Result<()> {
    let path = talos_patch_policy_backup_path();
    if !path.exists() {
        delete_registry_value(
            r"HKLM\SOFTWARE\Policies\Microsoft\Windows\WindowsUpdate",
            "SetDisableUXWUAccess",
        )?;
        delete_registry_value(
            r"HKLM\SOFTWARE\Policies\Microsoft\Windows\WindowsUpdate\AU",
            "NoAutoUpdate",
        )?;
        return Ok(());
    }
    let data = std::fs::read(&path).with_context(|| format!("read {}", path.display()))?;
    let backup: Value =
        serde_json::from_slice(&data).context("parse Windows Update policy backup")?;
    let values = backup.get("values").unwrap_or(&Value::Null);
    restore_registry_dword(
        r"HKLM\SOFTWARE\Policies\Microsoft\Windows\WindowsUpdate",
        "SetDisableUXWUAccess",
        values.get("setDisableUxWuAccess"),
    )?;
    restore_registry_dword(
        r"HKLM\SOFTWARE\Policies\Microsoft\Windows\WindowsUpdate\AU",
        "NoAutoUpdate",
        values.get("noAutoUpdate"),
    )?;
    Ok(())
}

#[cfg(target_os = "windows")]
fn restore_registry_dword(key: &str, name: &str, value: Option<&Value>) -> Result<()> {
    match value.and_then(Value::as_u64) {
        Some(number) => set_registry_dword(key, name, number as u32),
        None => delete_registry_value(key, name),
    }
}

#[cfg(target_os = "windows")]
fn set_registry_dword(key: &str, name: &str, value: u32) -> Result<()> {
    let status = std::process::Command::new("reg.exe")
        .args([
            "add",
            key,
            "/v",
            name,
            "/t",
            "REG_DWORD",
            "/d",
            &value.to_string(),
            "/f",
        ])
        .status()
        .with_context(|| format!("set registry value {key}\\{name}"))?;
    if !status.success() {
        anyhow::bail!("reg.exe add failed for {key}\\{name}: {status}");
    }
    Ok(())
}

#[cfg(target_os = "windows")]
fn delete_registry_value(key: &str, name: &str) -> Result<()> {
    let status = std::process::Command::new("reg.exe")
        .args(["delete", key, "/v", name, "/f"])
        .status()
        .with_context(|| format!("delete registry value {key}\\{name}"))?;
    if !status.success() {
        // Missing values return a failure exit code. That is acceptable while restoring defaults.
        warn!(key, name, status = %status, "registry value delete did not succeed");
    }
    Ok(())
}

#[cfg(target_os = "windows")]
fn read_registry_dword(key: &str, name: &str) -> Result<Option<u32>> {
    let output = std::process::Command::new("reg.exe")
        .args(["query", key, "/v", name])
        .output()
        .with_context(|| format!("query registry value {key}\\{name}"))?;
    if !output.status.success() {
        return Ok(None);
    }
    let text = String::from_utf8_lossy(&output.stdout);
    for line in text.lines() {
        if !line.contains(name) {
            continue;
        }
        let parts = line.split_whitespace().collect::<Vec<_>>();
        if let Some(raw) = parts.last() {
            let trimmed = raw.trim_start_matches("0x");
            if let Ok(value) = u32::from_str_radix(trimmed, 16) {
                return Ok(Some(value));
            }
            if let Ok(value) = raw.parse::<u32>() {
                return Ok(Some(value));
            }
        }
    }
    Ok(None)
}

#[cfg(target_os = "windows")]
fn execute_patch_job_windows(
    job: &PatchRemediationJob,
    progress_reporter: Option<&PatchProgressReporter>,
) -> Result<PatchExecutionOutcome> {
    use windows::{
        core::{BSTR, HSTRING},
        Win32::{
            Foundation::{RPC_E_CHANGED_MODE, VARIANT_BOOL},
            System::{
                Com::{
                    CLSIDFromProgID, CoCreateInstance, CoInitializeEx, CoUninitialize,
                    CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
                },
                UpdateAgent::{
                    IUpdate, IUpdateCollection, IUpdateSearcher, IUpdateSession, ServerSelection,
                },
            },
        },
    };

    struct ComApartmentGuard {
        should_uninitialize: bool,
    }

    impl Drop for ComApartmentGuard {
        fn drop(&mut self) {
            if self.should_uninitialize {
                unsafe { CoUninitialize() };
            }
        }
    }

    #[derive(Clone)]
    struct Candidate {
        update: IUpdate,
        identity: PatchUpdateIdentity,
        title: String,
        kb_article: Option<String>,
        requires_reboot: bool,
    }

    fn selected_update_progress_values(
        selected: &[Candidate],
        phase: &'static str,
        current_update_index: Option<i32>,
        current_update_percent: Option<i32>,
        completed_before_current: bool,
    ) -> Vec<Value> {
        selected
            .iter()
            .enumerate()
            .map(|(index, candidate)| {
                let index_i32 = index as i32;
                let state = match current_update_index {
                    Some(current) if index_i32 < current => {
                        if phase == "installing" && completed_before_current {
                            "installed"
                        } else {
                            "downloaded"
                        }
                    }
                    Some(current) if index_i32 == current => phase,
                    Some(_) => "queued",
                    None => phase,
                };
                json!({
                    "updateKey": candidate.identity.update_key.clone(),
                    "title": candidate.title.clone(),
                    "kbArticle": candidate.kb_article.clone(),
                    "index": index,
                    "state": state,
                    "percent": if current_update_index == Some(index_i32) {
                        current_update_percent.map(|value| value.clamp(0, 100))
                    } else if current_update_index.map(|current| index_i32 < current).unwrap_or(false) {
                        Some(100)
                    } else {
                        Some(0)
                    }
                })
            })
            .collect()
    }

    fn final_update_progress_values(
        selected: &[Candidate],
        evidence_updates: &[Value],
    ) -> Vec<Value> {
        selected
            .iter()
            .enumerate()
            .map(|(index, candidate)| {
                let evidence = evidence_updates.iter().find(|update| {
                    update
                        .get("updateKey")
                        .and_then(Value::as_str)
                        .map(|key| key == candidate.identity.update_key)
                        .unwrap_or(false)
                });
                let installed = evidence
                    .and_then(|update| update.get("installed"))
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                let downloaded = evidence
                    .and_then(|update| update.get("downloaded"))
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                let has_result = evidence
                    .and_then(|update| update.get("result"))
                    .and_then(Value::as_str)
                    .is_some();
                let state = if installed {
                    "installed"
                } else if downloaded || has_result {
                    "failed"
                } else {
                    "queued"
                };
                json!({
                    "updateKey": candidate.identity.update_key.clone(),
                    "title": candidate.title.clone(),
                    "kbArticle": candidate.kb_article.clone(),
                    "index": index,
                    "state": state,
                    "percent": if installed { 100 } else { 0 }
                })
            })
            .collect()
    }

    fn progress_summary(
        selected_len: usize,
        downloaded: usize,
        installed: usize,
        failed: usize,
        reboot_required: bool,
    ) -> Value {
        json!({
            "matched": selected_len,
            "downloaded": downloaded,
            "installed": installed,
            "failed": failed,
            "skipped": 0,
            "rebootRequired": reboot_required
        })
    }

    fn variant_true(value: VARIANT_BOOL) -> bool {
        value.0 != 0
    }

    fn result_code_label(code: i32) -> &'static str {
        match code {
            0 => "not_started",
            1 => "in_progress",
            2 => "succeeded",
            3 => "succeeded_with_errors",
            4 => "failed",
            5 => "aborted",
            _ => "unknown",
        }
    }

    fn create_update_collection() -> Result<IUpdateCollection> {
        unsafe {
            let clsid = CLSIDFromProgID(&HSTRING::from("Microsoft.Update.UpdateColl"))?;
            let collection: IUpdateCollection =
                CoCreateInstance(&clsid, None, CLSCTX_INPROC_SERVER)?;
            Ok(collection)
        }
    }

    fn read_candidate(update: IUpdate) -> Result<Candidate> {
        unsafe {
            let title = update.Title()?.to_string();
            let kb_article = update.KBArticleIDs().ok().and_then(|ids| {
                ids.get_Item(0).ok().map(|value| {
                    let text = value.to_string();
                    if text.to_ascii_uppercase().starts_with("KB") {
                        text
                    } else {
                        format!("KB{text}")
                    }
                })
            });
            let requires_reboot = update
                .InstallationBehavior()
                .ok()
                .and_then(|behavior| behavior.RebootBehavior().ok())
                .map(|value| value.0 != 0)
                .unwrap_or(false);
            let identity = PatchUpdateIdentity {
                update_key: build_patch_update_key(&title, kb_article.as_deref()),
                title_norm: normalize_patch_text(&title),
                kb_norm: normalize_patch_text(kb_article.as_deref().unwrap_or("")),
            };
            Ok(Candidate {
                update,
                identity,
                title,
                kb_article,
                requires_reboot,
            })
        }
    }

    fn query_candidates(
        searcher: &IUpdateSearcher,
        label: &str,
        criteria: &str,
    ) -> Result<Vec<Candidate>> {
        unsafe {
            let search_result = searcher
                .Search(&BSTR::from(criteria))
                .with_context(|| format!("WUA search failed for {label}: {criteria}"))?;
            let updates = search_result
                .Updates()
                .with_context(|| format!("WUA updates collection read failed for {label}"))?;
            let count = updates
                .Count()
                .with_context(|| format!("WUA updates count read failed for {label}"))?
                .max(0);

            let mut candidates = Vec::with_capacity(count as usize);
            for idx in 0..count {
                candidates.push(read_candidate(updates.get_Item(idx).with_context(
                    || format!("WUA update item read failed for {label} at {idx}"),
                )?)?);
            }
            Ok(candidates)
        }
    }

    fn extend_candidates_unique(target: &mut Vec<Candidate>, candidates: Vec<Candidate>) {
        let mut seen = target
            .iter()
            .map(|candidate| candidate.identity.update_key.clone())
            .collect::<HashSet<_>>();

        for candidate in candidates {
            if seen.insert(candidate.identity.update_key.clone()) {
                target.push(candidate);
            }
        }
    }

    let requested = requested_update_keys(job);
    let behavior = reboot_behavior(job);
    let download_only = is_download_only_job(job);

    unsafe {
        let hr = CoInitializeEx(None, COINIT_MULTITHREADED);
        let _guard = if hr.is_ok() {
            ComApartmentGuard {
                should_uninitialize: true,
            }
        } else if hr == RPC_E_CHANGED_MODE {
            ComApartmentGuard {
                should_uninitialize: false,
            }
        } else {
            anyhow::bail!("CoInitializeEx failed: {hr:?}");
        };

        let session_clsid = CLSIDFromProgID(&HSTRING::from("Microsoft.Update.Session"))?;
        let session: IUpdateSession = CoCreateInstance(&session_clsid, None, CLSCTX_INPROC_SERVER)?;
        let searcher = session.CreateUpdateSearcher()?;
        let _ = searcher.SetServerSelection(ServerSelection(0));
        let _ = searcher.SetOnline(VARIANT_BOOL(-1));
        let mut candidates =
            query_candidates(&searcher, "all_pending", "IsInstalled=0 and IsHidden=0")?;
        let upgrade_candidates = query_candidates(
            &searcher,
            "upgrade_pending",
            &format!(
                "IsInstalled=0 and IsHidden=0 and CategoryIDs contains '{}'",
                WU_UPGRADES_CATEGORY_ID
            ),
        )
        .unwrap_or_else(|error| {
            warn!(%error, "WU upgrade candidate query failed");
            Vec::new()
        });
        extend_candidates_unique(&mut candidates, upgrade_candidates);

        let mut selected = Vec::new();
        let mut evidence_updates = Vec::new();
        for candidate in candidates {
            let matched = update_matches_requested(&candidate.identity, &requested);
            if matched {
                selected.push(candidate.clone());
            }
            evidence_updates.push(json!({
                "updateKey": candidate.identity.update_key,
                "title": candidate.title,
                "kbArticle": candidate.kb_article,
                "matched": matched,
                "downloaded": false,
                "installed": false,
                "resultCode": null,
                "result": null,
                "hresult": null,
                "requiresReboot": candidate.requires_reboot
            }));
        }

        for requested_key in &requested {
            if !selected.iter().any(|candidate| {
                update_matches_requested(
                    &candidate.identity,
                    &HashSet::from([requested_key.clone()]),
                )
            }) {
                evidence_updates.push(json!({
                    "updateKey": requested_key,
                    "title": null,
                    "kbArticle": null,
                    "matched": false,
                    "downloaded": false,
                    "installed": false,
                    "resultCode": null,
                    "result": "not_found",
                    "hresult": null,
                    "requiresReboot": false
                }));
            }
        }

        if selected.is_empty() {
            return Ok(PatchExecutionOutcome {
                status: "completed",
                evidence: json!({
                    "phase": "completed",
                    "job": job_summary(job),
                    "updates": evidence_updates,
                    "summary": {
                        "matched": 0,
                        "downloaded": 0,
                        "installed": 0,
                        "failed": 0,
                        "skipped": requested.len(),
                        "rebootRequired": false
                    },
                    "error": null
                }),
                force_reboot_after_report: false,
            });
        }

        let selected_collection = create_update_collection()?;
        for candidate in &selected {
            if !variant_true(candidate.update.EulaAccepted().unwrap_or(VARIANT_BOOL(0))) {
                let _ = candidate.update.AcceptEula();
            }
            selected_collection.Add(&candidate.update)?;
        }

        let searching_snapshot = PatchProgressSnapshot {
            phase: "searching",
            overall_percent: 0,
            phase_percent: 100,
            current_update_index: None,
            current_update_percent: None,
        };
        send_patch_job_progress(
            progress_reporter,
            job,
            "searching",
            &selected_update_progress_values(&selected, "queued", None, None, false),
            progress_summary(selected.len(), 0, 0, 0, false),
            &searching_snapshot,
        );

        let downloader = session.CreateUpdateDownloader()?;
        downloader.SetUpdates(&selected_collection)?;

        let downloading_snapshot = PatchProgressSnapshot {
            phase: "downloading",
            overall_percent: 0,
            phase_percent: 0,
            current_update_index: None,
            current_update_percent: None,
        };
        let downloading_updates =
            selected_update_progress_values(&selected, "downloading", None, None, false);
        let downloading_summary = progress_summary(selected.len(), 0, 0, 0, false);
        send_patch_job_progress(
            progress_reporter,
            job,
            "downloading",
            &downloading_updates,
            downloading_summary.clone(),
            &downloading_snapshot,
        );

        let download_heartbeat = start_patch_job_progress_heartbeat(
            progress_reporter,
            job,
            "downloading",
            &downloading_updates,
            downloading_summary,
            &downloading_snapshot,
        );
        let download_result_raw = downloader.Download();
        drop(download_heartbeat);
        let download_result = match download_result_raw {
            Ok(result) => result,
            Err(error) => {
                let message = format!("WUA download failed: {error}");
                let failure_snapshot = PatchProgressSnapshot {
                    phase: "downloading",
                    overall_percent: 100,
                    phase_percent: 100,
                    current_update_index: None,
                    current_update_percent: None,
                };
                send_patch_job_progress_with_status(
                    progress_reporter,
                    job,
                    "failed",
                    "downloading",
                    &selected_update_progress_values(&selected, "failed", None, None, false),
                    progress_summary(selected.len(), 0, 0, selected.len(), false),
                    &failure_snapshot,
                    Some(&message),
                );
                return Ok(failed_outcome(
                    job,
                    "downloading",
                    message,
                    evidence_updates,
                ));
            }
        };
        let download_code = download_result.ResultCode()?.0;

        if download_only {
            let download_completed = download_code == 2;
            let downloaded_count = if download_completed {
                selected.len()
            } else {
                0
            };
            let failed_count = if download_completed {
                0
            } else {
                selected.len()
            };
            let final_state = if download_completed {
                "downloaded"
            } else {
                "failed"
            };
            let finalizing_snapshot = PatchProgressSnapshot {
                phase: "finalizing",
                overall_percent: 100,
                phase_percent: 100,
                current_update_index: None,
                current_update_percent: None,
            };

            for candidate in &selected {
                for update in &mut evidence_updates {
                    if update
                        .get("updateKey")
                        .and_then(Value::as_str)
                        .map(|key| key == candidate.identity.update_key)
                        .unwrap_or(false)
                    {
                        update["downloaded"] = json!(download_completed);
                        update["installed"] = json!(false);
                        update["resultCode"] = json!(download_code);
                        update["result"] = json!(result_code_label(download_code));
                        update["hresult"] = Value::Null;
                    }
                }
            }

            let status = if download_completed {
                "completed"
            } else {
                "failed"
            };
            send_patch_job_progress_with_status(
                progress_reporter,
                job,
                status,
                "finalizing",
                &selected_update_progress_values(&selected, final_state, None, None, false),
                progress_summary(selected.len(), downloaded_count, 0, failed_count, false),
                &finalizing_snapshot,
                if download_completed {
                    None
                } else {
                    Some("One or more updates failed to download")
                },
            );

            return Ok(PatchExecutionOutcome {
                status,
                evidence: json!({
                    "phase": if download_completed { "completed" } else { "failed" },
                    "mode": "download",
                    "job": job_summary(job),
                    "updates": evidence_updates,
                    "summary": {
                        "matched": selected.len(),
                        "downloaded": downloaded_count,
                        "installed": 0,
                        "failed": failed_count,
                        "skipped": 0,
                        "rebootRequired": false,
                        "downloadResultCode": download_code,
                        "downloadResult": result_code_label(download_code)
                    },
                    "error": if download_completed { Value::Null } else { json!("One or more updates failed to download") }
                }),
                force_reboot_after_report: false,
            });
        }

        let installer = session.CreateUpdateInstaller()?;
        installer.SetUpdates(&selected_collection)?;
        let download_succeeded = download_code == 2 || download_code == 3;
        for candidate in &selected {
            for update in &mut evidence_updates {
                if update
                    .get("updateKey")
                    .and_then(Value::as_str)
                    .map(|key| key == candidate.identity.update_key)
                    .unwrap_or(false)
                {
                    update["downloaded"] = json!(download_succeeded);
                }
            }
        }

        let installing_snapshot = PatchProgressSnapshot {
            phase: "installing",
            overall_percent: 50,
            phase_percent: 0,
            current_update_index: None,
            current_update_percent: None,
        };
        let installing_updates =
            selected_update_progress_values(&selected, "installing", None, None, true);
        let installing_summary = progress_summary(
            selected.len(),
            if download_succeeded {
                selected.len()
            } else {
                0
            },
            0,
            0,
            false,
        );
        send_patch_job_progress(
            progress_reporter,
            job,
            "installing",
            &installing_updates,
            installing_summary.clone(),
            &installing_snapshot,
        );

        let install_heartbeat = start_patch_job_progress_heartbeat(
            progress_reporter,
            job,
            "installing",
            &installing_updates,
            installing_summary,
            &installing_snapshot,
        );
        let install_result_raw = installer.Install();
        drop(install_heartbeat);
        let install_result = match install_result_raw {
            Ok(result) => result,
            Err(error) => {
                let message = format!("WUA install failed: {error}");
                let failure_snapshot = PatchProgressSnapshot {
                    phase: "installing",
                    overall_percent: 100,
                    phase_percent: 100,
                    current_update_index: None,
                    current_update_percent: None,
                };
                send_patch_job_progress_with_status(
                    progress_reporter,
                    job,
                    "failed",
                    "installing",
                    &selected_update_progress_values(&selected, "failed", None, None, false),
                    progress_summary(
                        selected.len(),
                        if download_succeeded {
                            selected.len()
                        } else {
                            0
                        },
                        0,
                        selected.len(),
                        false,
                    ),
                    &failure_snapshot,
                    Some(&message),
                );
                return Ok(failed_outcome(job, "installing", message, evidence_updates));
            }
        };

        let install_code = install_result.ResultCode()?.0;
        let reboot_required = variant_true(install_result.RebootRequired()?);
        let mut installed_count = 0usize;
        let mut failed_count = 0usize;

        for (idx, candidate) in selected.iter().enumerate() {
            let per_update = install_result.GetUpdateResult(idx as i32);
            let (result_code, hresult, installed) = match per_update {
                Ok(result) => {
                    let code = result
                        .ResultCode()
                        .map(|value| value.0)
                        .unwrap_or(install_code);
                    let hresult = result.HResult().unwrap_or(0);
                    let installed = code == 2 || code == 3;
                    (code, hresult, installed)
                }
                Err(error) => (4, error.code().0, false),
            };
            if installed {
                installed_count += 1;
            } else {
                failed_count += 1;
            }

            for update in &mut evidence_updates {
                if update
                    .get("updateKey")
                    .and_then(Value::as_str)
                    .map(|key| key == candidate.identity.update_key)
                    .unwrap_or(false)
                {
                    update["downloaded"] = json!(download_code == 2 || download_code == 3);
                    update["installed"] = json!(installed);
                    update["resultCode"] = json!(result_code);
                    update["result"] = json!(result_code_label(result_code));
                    update["hresult"] = json!(hresult);
                }
            }
        }

        let status = if failed_count == 0 {
            "completed"
        } else {
            "failed"
        };
        let terminal_error = if status == "completed" {
            None
        } else {
            Some("One or more updates failed to install")
        };
        let finalizing_snapshot = PatchProgressSnapshot {
            phase: "finalizing",
            overall_percent: 100,
            phase_percent: 100,
            current_update_index: None,
            current_update_percent: None,
        };
        send_patch_job_progress_with_status(
            progress_reporter,
            job,
            status,
            "finalizing",
            &final_update_progress_values(&selected, &evidence_updates),
            progress_summary(
                selected.len(),
                if download_code == 2 || download_code == 3 {
                    selected.len()
                } else {
                    0
                },
                installed_count,
                failed_count,
                reboot_required,
            ),
            &finalizing_snapshot,
            terminal_error,
        );
        let force_reboot_after_report =
            reboot_required && behavior == "force" && status == "completed";

        Ok(PatchExecutionOutcome {
            status,
            evidence: json!({
                "phase": if status == "completed" { "completed" } else { "failed" },
                "job": job_summary(job),
                "updates": evidence_updates,
                "summary": {
                    "matched": selected.len(),
                    "downloaded": if download_code == 2 || download_code == 3 { selected.len() } else { 0 },
                    "installed": installed_count,
                    "failed": failed_count,
                    "skipped": 0,
                    "rebootRequired": reboot_required,
                    "downloadResultCode": download_code,
                    "downloadResult": result_code_label(download_code),
                    "installResultCode": install_code,
                    "installResult": result_code_label(install_code),
                    "rebootBehavior": behavior
                },
                "error": if status == "completed" { Value::Null } else { json!("One or more updates failed to install") }
            }),
            force_reboot_after_report,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patch_command_timeout_env_value_uses_default_and_clamps() {
        assert_eq!(
            patch_command_timeout_from_env_value(None),
            Duration::from_secs(DEFAULT_PATCH_COMMAND_TIMEOUT_SECS)
        );
        assert_eq!(
            patch_command_timeout_from_env_value(Some("bad")),
            Duration::from_secs(DEFAULT_PATCH_COMMAND_TIMEOUT_SECS)
        );
        assert_eq!(
            patch_command_timeout_from_env_value(Some("1")),
            Duration::from_secs(MIN_PATCH_COMMAND_TIMEOUT_SECS)
        );
        assert_eq!(
            patch_command_timeout_from_env_value(Some("999999")),
            Duration::from_secs(MAX_PATCH_COMMAND_TIMEOUT_SECS)
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_program_timeout_fails_quickly() {
        let args = vec!["-c".to_string(), "sleep 2".to_string()];
        let error = run_linux_program_blocking_with_timeout(
            "sh",
            &args,
            "test sleep",
            &[0],
            Duration::from_millis(10),
        )
        .unwrap_err();

        assert!(format!("{error:#}").contains("timed out"));
    }

    #[test]
    fn build_update_key_normalizes_title_and_kb() {
        assert_eq!(
            build_patch_update_key("  Security   Update  ", Some("KB5000001")),
            "security update|kb5000001"
        );
    }

    #[test]
    fn action_plan_download_builds_download_only_job() {
        let plan = PatchActionPlan {
            schema_version: 1,
            generated_at: "2026-05-29T12:00:00Z".to_string(),
            organization_id: Some("org-1".to_string()),
            agent_id: "agent-1".to_string(),
            policy_id: Some("policy-1".to_string()),
            managed_mode: true,
            native_windows_update_control: false,
            next_check_in_at: None,
            actions: Vec::new(),
        };
        let item = PatchActionPlanItem {
            operation_id: "op-1".to_string(),
            action: "download".to_string(),
            update_keys: vec!["macos sonoma 14.5|".to_string()],
            window: None,
            not_before: None,
            deadline_at: None,
            forced: false,
            reason: "test".to_string(),
            metadata: json!({}),
        };

        let job = action_item_to_patch_job(&plan, &item, "download");

        assert_eq!(job.metadata["mode"], "download");
        assert_eq!(job.metadata["downloadOnly"], true);
        assert_eq!(job.metadata["rebootBehavior"], "allow");
        assert_eq!(job.metadata["updateKeys"][0], "macos sonoma 14.5|");
    }

    #[test]
    fn requested_update_matches_exact_key() {
        let update = parse_update_key("security update|kb5000001");
        let requested = HashSet::from(["security update|kb5000001".to_string()]);
        assert!(update_matches_requested(&update, &requested));
    }

    #[test]
    fn requested_update_matches_title_when_one_side_has_no_kb() {
        let update = parse_update_key("security update|");
        let requested = HashSet::from(["security update|kb5000001".to_string()]);
        assert!(update_matches_requested(&update, &requested));
    }

    #[test]
    fn requested_update_rejects_different_title() {
        let update = parse_update_key("cumulative update|kb5000001");
        let requested = HashSet::from(["security update|kb5000001".to_string()]);
        assert!(!update_matches_requested(&update, &requested));
    }

    #[test]
    fn patch_scan_progress_payload_uses_scan_event_shape() {
        let payload =
            patch_scan_progress_payload("org-1", "agent-1", "operation-1", "running", None, None);

        assert_eq!(payload["eventType"], "patch.scan.progress");
        assert_eq!(payload["organizationId"], "org-1");
        assert_eq!(payload["agentId"], "agent-1");
        assert_eq!(payload["jobId"], "operation-1");
        assert_eq!(payload["commandId"], "operation-1");
        assert_eq!(payload["status"], "running");
        assert_eq!(payload["phase"], "scanning");
        assert_eq!(payload["summary"]["snapshotRequested"], true);
    }

    #[test]
    fn snapshot_pending_update_count_reads_full_snapshot_envelope() {
        let payload = json!({
            "snapshot": {
                "collection": {
                    "operating_system": {
                        "updates": {
                            "windows_update": {
                                "pending_count": 2,
                                "pending_updates": [
                                    { "title": "Update 1" },
                                    { "title": "Update 2" }
                                ]
                            }
                        }
                    }
                }
            }
        });

        assert_eq!(snapshot_pending_update_count(&payload), 2);
    }

    #[test]
    fn snapshot_pending_update_count_falls_back_to_summary_count() {
        let payload = json!({
            "snapshot": {
                "collection": {
                    "software": {
                        "windows_updates": {
                            "pending_count": 5
                        }
                    }
                }
            }
        });

        assert_eq!(snapshot_pending_update_count(&payload), 5);
    }

    #[test]
    fn parses_apt_upgradable_candidates() {
        let updates = parse_apt_upgradable_candidates(
            "Listing... Done\nopenssl/noble-updates 3.0.13-0ubuntu3.6 amd64 [upgradable from: 3.0.13-0ubuntu3.5]\n",
            true,
        );

        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0].package, "openssl");
        assert_eq!(updates[0].source.as_deref(), Some("noble-updates"));
        assert_eq!(updates[0].target_version, "3.0.13-0ubuntu3.6");
        assert_eq!(
            updates[0].current_version.as_deref(),
            Some("3.0.13-0ubuntu3.5")
        );
        assert_eq!(updates[0].architecture.as_deref(), Some("amd64"));
        assert_eq!(updates[0].title, "openssl 3.0.13-0ubuntu3.6");
        assert_eq!(updates[0].identity.update_key, "openssl 3.0.13-0ubuntu3.6|");
        assert!(updates[0].requires_reboot);
    }

    #[test]
    fn apt_candidate_matches_package_when_requested_version_drifted() {
        let candidates = parse_apt_upgradable_candidates(
            "Listing... Done\nopenssl/noble-updates 3.0.13-0ubuntu3.7 amd64 [upgradable from: 3.0.13-0ubuntu3.5]\n",
            false,
        );
        let requested = HashSet::from([build_patch_update_key("openssl 3.0.13-0ubuntu3.6", None)]);

        assert!(apt_candidate_matches_requested(&candidates[0], &requested));
    }

    #[test]
    fn apt_command_args_use_safe_noninteractive_upgrade_shape() {
        let candidates = parse_apt_upgradable_candidates(
            "Listing... Done\nopenssl/noble-updates 3.0.13-0ubuntu3.6 amd64 [upgradable from: 3.0.13-0ubuntu3.5]\n",
            false,
        );
        let specs = apt_package_specs(&candidates);
        let download_args = apt_get_download_args(&specs);
        let install_args = apt_get_install_args(&specs);

        assert!(download_args.contains(&"DPkg::Lock::Timeout=300".to_string()));
        assert!(download_args.contains(&"Dpkg::Options::=--force-confdef".to_string()));
        assert!(download_args.contains(&"Dpkg::Options::=--force-confold".to_string()));
        assert!(download_args.contains(&"--download-only".to_string()));
        assert!(download_args.contains(&"--only-upgrade".to_string()));
        assert!(download_args.contains(&"openssl=3.0.13-0ubuntu3.6".to_string()));

        assert!(!install_args.contains(&"--download-only".to_string()));
        assert!(install_args.contains(&"--only-upgrade".to_string()));
        assert!(install_args.contains(&"openssl=3.0.13-0ubuntu3.6".to_string()));
    }

    #[test]
    fn parses_dnf_yum_check_update_candidates() {
        let updates = parse_dnf_yum_check_update_candidates(
            "Last metadata expiration check: 0:01:00 ago\n\
NetworkManager.x86_64                1:1.54.3-2.fc43             updates\n\
ca-certificates.noarch               2025.2.80_v9.0.304-1.2.fc43 updates\n\
glibc.x86_64                         2.42-12.fc43                updates\n\
\n\
Obsoleting Packages\n\
gnupg2.x86_64                        2.4.9-5.fc43                updates\n\
    gnupg2.x86_64                    2.4.8-4.fc43                oldrepo\n",
            false,
        );

        assert_eq!(updates.len(), 3);
        assert_eq!(updates[0].package, "NetworkManager");
        assert_eq!(updates[0].architecture.as_deref(), Some("x86_64"));
        assert_eq!(updates[0].target_version, "1:1.54.3-2.fc43");
        assert_eq!(updates[0].source.as_deref(), Some("updates"));
        assert_eq!(updates[0].title, "NetworkManager 1:1.54.3-2.fc43");
        assert_eq!(
            updates[0].identity.update_key,
            "networkmanager 1:1.54.3-2.fc43|"
        );
        assert_eq!(updates[1].package, "ca-certificates");
        assert_eq!(updates[1].architecture.as_deref(), Some("noarch"));
        assert!(updates[2].requires_reboot);
    }

    #[test]
    fn parses_macos_softwareupdate_candidates() {
        let updates = parse_macos_softwareupdate_candidates(
            "Software Update Tool\n\n* Label: macOS Sonoma 14.5-23F79\n    Title: macOS Sonoma 14.5, Version: 14.5, Size: 3846061KiB, Recommended: YES, Action: restart,\n",
        );

        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0].label, "macOS Sonoma 14.5-23F79");
        assert_eq!(updates[0].title, "macOS Sonoma 14.5");
        assert_eq!(updates[0].version.as_deref(), Some("14.5"));
        assert!(updates[0].recommended);
        assert!(updates[0].requires_reboot);
        assert_eq!(updates[0].identity.update_key, "macos sonoma 14.5|");
    }

    #[test]
    fn parses_macos_softwareupdate_titles_with_commas() {
        let updates = parse_macos_softwareupdate_candidates(
            "Software Update Tool\n\n* Label: Example App Update-1.2\n    Title: Example App, Security Update, Version: 1.2, Size: 12345KiB, Recommended: YES, Action: none,\n",
        );

        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0].title, "Example App, Security Update");
        assert_eq!(updates[0].version.as_deref(), Some("1.2"));
        assert_eq!(
            updates[0].identity.update_key,
            "example app, security update|"
        );
    }

    #[test]
    fn parses_macos_softwareupdate_candidates_from_combined_output() {
        let output = combined_macos_softwareupdate_output(
            b"Software Update Tool\n\nFinding available software\n",
            b"* Label: Safari17.5.1-17618.2.12.111.5\n    Title: Safari, Version: 17.5.1, Size: 120000KiB, Recommended: YES, Action: none,\n",
        );
        let updates = parse_macos_softwareupdate_candidates(&output);

        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0].label, "Safari17.5.1-17618.2.12.111.5");
        assert_eq!(updates[0].title, "Safari");
        assert_eq!(updates[0].version.as_deref(), Some("17.5.1"));
        assert!(updates[0].recommended);
        assert!(!updates[0].requires_reboot);
    }

    #[test]
    fn parses_macos_softwareupdate_candidates_case_insensitively() {
        let updates = parse_macos_softwareupdate_candidates(
            "software update tool\n\n* label: Safari17.5.1-17618.2.12.111.5\n    title: Safari, version: 17.5.1, size: 120000KiB, recommended: YES, action: restart,\n",
        );

        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0].label, "Safari17.5.1-17618.2.12.111.5");
        assert_eq!(updates[0].title, "Safari");
        assert_eq!(updates[0].version.as_deref(), Some("17.5.1"));
        assert_eq!(updates[0].size.as_deref(), Some("120000KiB"));
        assert!(updates[0].recommended);
        assert!(updates[0].requires_reboot);
    }

    #[test]
    fn filters_macos_patch_candidates_to_latest_os_version() {
        let updates = parse_macos_softwareupdate_candidates(
            "Software Update Tool\n\n* Label: macOS Tahoe 26.5.1-25F5057\n    Title: macOS Tahoe 26.5.1, Version: 26.5.1, Size: 8000000KiB, Recommended: YES, Action: restart,\n* Label: macOS Ventura 13.7.8-22H730\n    Title: macOS Ventura 13.7.8, Version: 13.7.8, Size: 4000000KiB, Recommended: YES, Action: restart,\n* Label: Safari18.5-18621\n    Title: Safari, Version: 18.5, Size: 120000KiB, Recommended: YES, Action: none,\n* Label: macOS Security Response 14.5-a\n    Title: macOS Security Response 14.5, Version: 14.5, Size: 1000KiB, Recommended: YES, Action: restart,\n",
        );

        let titles = updates
            .iter()
            .map(|update| update.title.as_str())
            .collect::<Vec<_>>();
        assert!(titles.contains(&"macOS Tahoe 26.5.1"));
        assert!(titles.contains(&"Safari"));
        assert!(titles.contains(&"macOS Security Response 14.5"));
        assert!(!titles.contains(&"macOS Ventura 13.7.8"));
    }

    #[test]
    fn macos_softwareupdate_label_args_use_option_terminator() {
        let labels = ["Safari17.5.1-17618.2.12.111.5", "-odd-leading-dash"];

        assert_eq!(
            macos_softwareupdate_label_args("-d", labels.iter().copied()),
            vec![
                "-d",
                "--",
                "Safari17.5.1-17618.2.12.111.5",
                "-odd-leading-dash"
            ]
        );
    }

    #[test]
    fn macos_softwareupdate_install_args_agree_to_license() {
        let labels = ["macOS Sonoma 14.5-23F79", "-odd-leading-dash"];

        assert_eq!(
            macos_softwareupdate_install_label_args(labels.iter().copied()),
            vec![
                "--agree-to-license",
                "-i",
                "--",
                "macOS Sonoma 14.5-23F79",
                "-odd-leading-dash"
            ]
        );
    }

    #[test]
    fn macos_softwareupdate_owner_install_args_use_stdinpass_before_license_and_operation() {
        let labels = ["macOS Sonoma 14.5-23F79", "-odd-leading-dash"];

        assert_eq!(
            macos_softwareupdate_install_label_args_with_owner("talos", labels.iter().copied()),
            vec![
                "--user",
                "talos",
                "--stdinpass",
                "--agree-to-license",
                "-i",
                "--",
                "macOS Sonoma 14.5-23F79",
                "-odd-leading-dash"
            ]
        );
    }

    #[test]
    fn macos_redacts_stdinpass_from_failure_text() {
        let secrets = macos_stdin_secret_values(Some("super-secret-password\n"));
        let text = redact_macos_secret_values(
            "softwareupdate failed with super-secret-password in output",
            &secrets,
        );

        assert_eq!(text, "softwareupdate failed with [redacted] in output");
        assert!(!text.contains("super-secret-password"));
    }

    #[test]
    fn macos_diagnostic_truncation_preserves_utf8_boundary() {
        let value = format!("{}é", "a".repeat(11_999));
        let truncated = truncate_macos_diagnostic(&value);

        assert!(truncated.ends_with("..."));
        assert!(truncated.is_char_boundary(truncated.len()));
    }

    #[test]
    fn parses_macos_size_units_to_bytes() {
        assert_eq!(parse_macos_size_bytes("120000KiB"), Some(122_880_000));
        assert_eq!(parse_macos_size_bytes("1.5 GB"), Some(1_610_612_736));
        assert_eq!(parse_macos_size_bytes("2 MiB"), Some(2_097_152));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_storage_uses_existing_parent_for_missing_staging_path() {
        let missing = format!(
            "/tmp/talos-missing-staging-{}/Library/Updates",
            Uuid::new_v4()
        );
        let parent = nearest_existing_path(&missing);

        assert_eq!(parent, std::path::PathBuf::from("/tmp"));
    }

    #[test]
    fn macos_candidate_evidence_reports_matched_and_selected_state() {
        let candidates = parse_macos_softwareupdate_candidates(
            "Software Update Tool\n\n* Label: macOS Sonoma 14.5-23F79\n    Title: macOS Sonoma 14.5, Version: 14.5, Size: 3846061KiB, Recommended: YES, Action: restart,\n",
        );

        let selected = macos_candidate_evidence(&candidates[0], true);
        assert_eq!(selected.get("matched").and_then(Value::as_bool), Some(true));
        assert_eq!(
            selected.get("selected").and_then(Value::as_bool),
            Some(true)
        );
        assert_eq!(
            selected.get("result").and_then(Value::as_str),
            Some("queued")
        );

        let skipped = macos_candidate_evidence(&candidates[0], false);
        assert_eq!(skipped.get("matched").and_then(Value::as_bool), Some(false));
        assert_eq!(
            skipped.get("selected").and_then(Value::as_bool),
            Some(false)
        );
        assert_eq!(
            skipped.get("result").and_then(Value::as_str),
            Some("skipped")
        );
    }

    #[test]
    fn macos_requested_updates_missing_from_scan_are_reported() {
        let candidates = parse_macos_softwareupdate_candidates(
            "Software Update Tool\n\n* Label: Safari17.5.1-17618.2.12.111.5\n    Title: Safari, Version: 17.5.1, Size: 120000KiB, Recommended: YES, Action: none,\n",
        );
        let requested = HashSet::from([
            build_patch_update_key("Safari", None),
            build_patch_update_key("macOS Sonoma 14.5", None),
        ]);
        let mut evidence = candidates
            .iter()
            .map(|candidate| macos_candidate_evidence(candidate, true))
            .collect::<Vec<_>>();

        append_missing_macos_requested_update_evidence(&mut evidence, &candidates, &requested);

        assert_eq!(evidence.len(), 2);
        assert!(evidence.iter().any(|update| {
            update.get("updateKey").and_then(Value::as_str) == Some("safari|")
                && update.get("result").and_then(Value::as_str) == Some("queued")
        }));
        assert!(evidence.iter().any(|update| {
            update.get("updateKey").and_then(Value::as_str) == Some("macos sonoma 14.5|")
                && update.get("matched").and_then(Value::as_bool) == Some(false)
                && update.get("selected").and_then(Value::as_bool) == Some(false)
                && update.get("result").and_then(Value::as_str) == Some("not_found")
        }));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_softwareupdate_uses_process_wide_lock() {
        let first = macos_softwareupdate_lock() as *const _;
        let second = macos_softwareupdate_lock() as *const _;

        assert_eq!(first, second);
    }

    #[test]
    fn reboot_notice_deferral_state_stops_at_limit() {
        assert!(reboot_notice_can_defer(0, 4));
        assert_eq!(next_reboot_notice_deferral_count(3, 4), Some(4));
        assert!(!reboot_notice_can_defer(4, 4));
        assert_eq!(next_reboot_notice_deferral_count(4, 4), None);
    }

    #[test]
    fn immediate_reboot_commands_do_not_schedule_delayed_reboots() {
        let (windows_program, windows_args) = windows_immediate_reboot_command();
        assert_eq!(windows_program, "shutdown.exe");
        assert!(windows_args.contains(&"/t"));
        assert!(windows_args.contains(&"0"));
        assert!(!windows_args.contains(&"60"));
        assert!(!windows_args.contains(&"900"));

        let (macos_program, macos_args) = macos_immediate_reboot_command();
        assert_eq!(macos_program, "/sbin/shutdown");
        assert!(macos_args.contains(&"now"));
        assert!(!macos_args.contains(&"+1"));
        assert!(!macos_args.contains(&"+15"));
    }

    #[test]
    fn macos_download_only_outcome_does_not_require_reboot() {
        let candidates = parse_macos_softwareupdate_candidates(
            "Software Update Tool\n\n* Label: macOS Sonoma 14.5-23F79\n    Title: macOS Sonoma 14.5, Version: 14.5, Size: 3846061KiB, Recommended: YES, Action: restart,\n",
        );

        assert!(!macos_reboot_required_for_outcome(&candidates, 0));
        assert!(macos_reboot_required_for_outcome(&candidates, 1));
    }

    #[test]
    fn macos_command_failure_detail_preserves_stdout_and_stderr() {
        assert_eq!(
            macos_command_failure_detail("Downloading update\n", "Install failed\n"),
            "Downloading update\nInstall failed"
        );
        assert_eq!(
            macos_command_failure_detail("", "Only stderr\n"),
            "Only stderr"
        );
    }

    #[test]
    fn rpm_candidate_matches_package_when_requested_version_drifted() {
        let candidates =
            parse_dnf_yum_check_update_candidates("openssl.x86_64 1:3.5.4-3.fc43 updates\n", false);
        let requested = HashSet::from([build_patch_update_key("openssl 1:3.5.4-2.fc43", None)]);

        assert!(rpm_candidate_matches_requested(&candidates[0], &requested));
    }

    #[test]
    fn dnf_yum_command_args_use_upgrade_shape() {
        let candidates =
            parse_dnf_yum_check_update_candidates("openssl.x86_64 1:3.5.4-3.fc43 updates\n", false);
        let specs = rpm_package_specs(&candidates);
        let dnf_makecache = dnf_yum_makecache_args(LinuxPatchPackageManager::Dnf);
        let yum_makecache = dnf_yum_makecache_args(LinuxPatchPackageManager::Yum);
        let download_args = dnf_yum_download_args(&specs);
        let install_args = dnf_yum_install_args(&specs);

        assert_eq!(dnf_makecache, vec!["makecache", "--refresh"]);
        assert_eq!(yum_makecache, vec!["makecache"]);
        assert!(download_args.contains(&"-y".to_string()));
        assert!(download_args.contains(&"upgrade".to_string()));
        assert!(download_args.contains(&"--downloadonly".to_string()));
        assert!(download_args.contains(&"openssl".to_string()));

        assert!(install_args.contains(&"-y".to_string()));
        assert!(install_args.contains(&"upgrade".to_string()));
        assert!(!install_args.contains(&"--downloadonly".to_string()));
        assert!(install_args.contains(&"openssl".to_string()));
    }

    #[test]
    fn rpm_reboot_heuristic_flags_core_packages() {
        assert!(rpm_package_requires_reboot("kernel-core"));
        assert!(rpm_package_requires_reboot("systemd-libs"));
        assert!(rpm_package_requires_reboot("glibc"));
        assert!(rpm_package_requires_reboot("dbus-broker"));
        assert!(rpm_package_requires_reboot("rpm-libs"));
        assert!(rpm_package_requires_reboot("dnf5"));
        assert!(!rpm_package_requires_reboot("nano"));
    }
}
