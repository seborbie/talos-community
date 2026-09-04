use std::time::Duration;

#[cfg(target_os = "macos")]
use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::{
    sync::mpsc,
    time::{sleep, timeout},
};
use tokio_tungstenite::tungstenite::Message;
use tracing::{info, warn};
use uuid::Uuid;

use crate::patching;

const REMEDIATION_POLL_INTERVAL: Duration = Duration::from_secs(30);
const REMEDIATION_POLL_RESPONSE_TIMEOUT: Duration = Duration::from_secs(25);
const PATCH_INSTALL_INTENT_ID: &str = "talos.patch.install";
const MAX_OUTPUT_BYTES: usize = 32 * 1024;
const MAX_EVIDENCE_BYTES: usize = 32 * 1024;
const TRUNCATION_SUFFIX: &str = "\n...[truncated]";

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RemediationJobsEnvelope {
    pub request_id: String,
    pub jobs: Vec<RemediationJob>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RemediationJob {
    pub command_id: String,
    pub organization_id: String,
    pub agent_id: String,
    pub intent_id: String,
    #[serde(default)]
    pub decision_id: Option<String>,
    #[serde(default)]
    pub dedupe_key: Option<String>,
    pub requested_by: String,
    pub requested_at: String,
    pub approval_state: String,
    #[serde(default)]
    pub metadata: Value,
    #[serde(default)]
    pub steps: Vec<Value>,
    #[serde(default)]
    pub execution: Value,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RemediationJobsAvailablePayload {
    pub reason: Option<String>,
    pub requested_by: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RemediationJobsPollPayload {
    request_id: String,
    limit: u32,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RemediationJobUpdatePayload {
    command_id: String,
    status: String,
    step_index: i32,
    evidence: Value,
}

#[derive(Debug, Clone)]
struct CommandStep {
    step_index: i32,
    command: String,
    timeout_seconds: Option<u64>,
}

#[derive(Debug)]
struct StepResult {
    step_index: i32,
    command: String,
    exit_code: Option<i32>,
    output: String,
    status: &'static str,
}

pub(crate) fn start_remediation_manager(
    outbound_tx: mpsc::UnboundedSender<Message>,
    mut jobs_rx: mpsc::UnboundedReceiver<RemediationJobsEnvelope>,
    mut wake_rx: mpsc::UnboundedReceiver<()>,
) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(REMEDIATION_POLL_INTERVAL);
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

            let jobs = match poll_remediation_jobs(&outbound_tx, &mut jobs_rx).await {
                Ok(jobs) => jobs,
                Err(error) => {
                    warn!(%error, "remediation job poll failed");
                    continue;
                }
            };

            for job in jobs {
                if let Err(error) = run_remediation_job(&outbound_tx, job).await {
                    warn!(%error, "remediation job execution failed before status could be reported");
                }
            }
        }
    });
}

pub(crate) fn send_remediation_jobs_available_signal(
    wake_tx: &mpsc::UnboundedSender<()>,
    payload: RemediationJobsAvailablePayload,
) {
    info!(
        reason = ?payload.reason,
        requested_by = ?payload.requested_by,
        "remediation jobs available; waking remediation manager"
    );
    let _ = wake_tx.send(());
}

async fn poll_remediation_jobs(
    outbound_tx: &mpsc::UnboundedSender<Message>,
    jobs_rx: &mut mpsc::UnboundedReceiver<RemediationJobsEnvelope>,
) -> Result<Vec<RemediationJob>> {
    let request_id = Uuid::new_v4().to_string();
    send_envelope(
        outbound_tx,
        "remediation_jobs_poll",
        RemediationJobsPollPayload {
            request_id: request_id.clone(),
            limit: 1,
        },
    )?;

    tokio::time::timeout(REMEDIATION_POLL_RESPONSE_TIMEOUT, async {
        while let Some(payload) = jobs_rx.recv().await {
            if payload.request_id == request_id {
                return Ok(payload.jobs);
            }
            warn!(
                expected_request_id = %request_id,
                received_request_id = %payload.request_id,
                "discarding stale remediation jobs response"
            );
        }
        Err(anyhow::anyhow!("remediation jobs response channel closed"))
    })
    .await
    .context("remediation jobs poll timed out")?
}

async fn run_remediation_job(
    outbound_tx: &mpsc::UnboundedSender<Message>,
    job: RemediationJob,
) -> Result<()> {
    if job.intent_id == PATCH_INSTALL_INTENT_ID {
        send_remediation_job_update(
            outbound_tx,
            &job.command_id,
            "running",
            0,
            json!({
                "phase": "running",
                "job": job_summary(&job),
                "steps": [],
                "error": null
            }),
        )?;
        let patch_job = to_patch_job(&job);
        let reporter = patching::progress_reporter(outbound_tx, &patch_job);
        let outcome = tokio::task::spawn_blocking(move || {
            patching::execute_patch_job_blocking_with_progress(patch_job, Some(reporter))
        })
        .await
        .context("patch remediation task failed")?;
        send_remediation_job_update(
            outbound_tx,
            &job.command_id,
            outcome.status,
            0,
            outcome.evidence.clone(),
        )?;
        if outcome.force_reboot_after_report {
            match tokio::task::spawn_blocking(patching::schedule_forced_reboot_after_patch).await {
                Ok(Ok(schedule_evidence)) => {
                    let mut evidence = outcome.evidence;
                    evidence["rebootScheduled"] = json!(true);
                    evidence["rebootSchedule"] = schedule_evidence;
                    let _ = send_remediation_job_update(
                        outbound_tx,
                        &job.command_id,
                        outcome.status,
                        0,
                        evidence,
                    );
                }
                Ok(Err(error)) => {
                    warn!(%error, "failed to schedule forced reboot after remediation patch install")
                }
                Err(error) => warn!(%error, "forced reboot scheduling task failed"),
            }
        }
        return Ok(());
    }

    let steps = parse_steps(&job);
    // Step starts are safe with both API generations. Keep completed outcomes in one terminal
    // report so the API can project them atomically; older APIs treated any terminal step as a
    // terminal job and would close multi-step work prematurely.
    let outcome = execute_generic_steps(&job, &steps, |step| {
        send_remediation_job_update(
            outbound_tx,
            &job.command_id,
            "running",
            step.step_index,
            json!({
                "phase": "running",
                "stepIndex": step.step_index,
                "error": null
            }),
        )
    })
    .await?;
    let status = if outcome.iter().all(|step| step.status == "completed") {
        "completed"
    } else {
        "failed"
    };
    let final_step_index = outcome.last().map(|step| step.step_index).unwrap_or(0);
    let evidence = build_generic_job_evidence(&job, &outcome, status)?;
    send_remediation_job_update(
        outbound_tx,
        &job.command_id,
        status,
        final_step_index,
        evidence,
    )?;
    Ok(())
}

async fn execute_generic_steps<F>(
    job: &RemediationJob,
    steps: &[CommandStep],
    mut before_step: F,
) -> Result<Vec<StepResult>>
where
    F: FnMut(&CommandStep) -> Result<()>,
{
    let default_timeout = job
        .execution
        .get("timeoutSeconds")
        .and_then(Value::as_u64)
        .unwrap_or(300);
    let max_retries = job
        .execution
        .get("maxRetries")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let stop_on_failure = job
        .execution
        .get("stopOnFailure")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let mut results = Vec::new();

    for step in steps {
        before_step(step)?;
        let timeout_seconds = step.timeout_seconds.unwrap_or(default_timeout).max(1);
        let mut last_result = None;
        for attempt in 0..=max_retries {
            let result = execute_shell_command(&step.command, timeout_seconds).await;
            let status = if result.exit_code == Some(0) {
                "completed"
            } else {
                "failed"
            };
            last_result = Some(StepResult {
                step_index: step.step_index,
                command: step.command.clone(),
                exit_code: result.exit_code,
                output: truncate_output(result.output),
                status,
            });
            if status == "completed" {
                break;
            }
            if attempt < max_retries {
                sleep(Duration::from_millis(500)).await;
            }
        }
        let result = last_result.expect("step execution always records a result");
        let failed = result.status != "completed";
        results.push(result);
        if failed && stop_on_failure {
            break;
        }
    }

    Ok(results)
}

fn build_generic_job_evidence(
    job: &RemediationJob,
    outcome: &[StepResult],
    status: &str,
) -> Result<Value> {
    let mut evidence = json!({
        "phase": status,
        "job": job_summary(job),
        "steps": outcome.iter().map(|step| json!({
            "stepIndex": step.step_index,
            "command": step.command,
            "exitCode": step.exit_code,
            "output": step.output,
            "status": step.status
        })).collect::<Vec<_>>(),
        "error": if status == "completed" { Value::Null } else { json!("One or more remediation steps failed") }
    });
    bound_evidence_outputs(&mut evidence)?;
    Ok(evidence)
}

fn bound_evidence_outputs(evidence: &mut Value) -> Result<()> {
    loop {
        let encoded_len = serde_json::to_vec(evidence)
            .context("serialize remediation evidence for size validation")?
            .len();
        if encoded_len <= MAX_EVIDENCE_BYTES {
            return Ok(());
        }

        let steps = evidence
            .get_mut("steps")
            .and_then(Value::as_array_mut)
            .context("generic remediation evidence must contain a steps array")?;
        let Some((longest_index, longest_len)) = steps
            .iter()
            .enumerate()
            .filter_map(|(index, step)| {
                step.get("output")
                    .and_then(Value::as_str)
                    .map(|output| (index, output.len()))
            })
            .max_by_key(|(_, length)| *length)
        else {
            anyhow::bail!("generic remediation evidence exceeds the 32 KiB API limit");
        };
        if longest_len == 0 {
            anyhow::bail!("generic remediation evidence metadata exceeds the 32 KiB API limit");
        }

        let excess = encoded_len - MAX_EVIDENCE_BYTES;
        let target_len = longest_len.saturating_sub(excess.max(1));
        let output = steps[longest_index]
            .get("output")
            .and_then(Value::as_str)
            .context("generic remediation step output must be a string")?;
        steps[longest_index]["output"] = Value::String(truncate_utf8(output, target_len));
    }
}

fn parse_steps(job: &RemediationJob) -> Vec<CommandStep> {
    job.steps
        .iter()
        .enumerate()
        .filter_map(|(index, value)| {
            let command = value.get("command")?.as_str()?.trim().to_string();
            if command.is_empty() {
                return None;
            }
            Some(CommandStep {
                step_index: value
                    .get("stepIndex")
                    .or_else(|| value.get("step_index"))
                    .and_then(Value::as_i64)
                    .unwrap_or(index as i64) as i32,
                command,
                timeout_seconds: value
                    .get("timeoutSeconds")
                    .or_else(|| value.get("timeout_seconds"))
                    .and_then(Value::as_u64),
            })
        })
        .collect()
}

struct ShellCommandResult {
    output: String,
    exit_code: Option<i32>,
}

async fn execute_shell_command(command: &str, timeout_seconds: u64) -> ShellCommandResult {
    match timeout(
        Duration::from_secs(timeout_seconds),
        execute_shell_command_inner(command),
    )
    .await
    {
        Ok(result) => result,
        Err(_) => ShellCommandResult {
            output: format!("Command timed out after {timeout_seconds} seconds"),
            exit_code: Some(-1),
        },
    }
}

#[cfg(target_os = "windows")]
async fn execute_shell_command_inner(command: &str) -> ShellCommandResult {
    use tokio::process::Command;

    command_output(
        Command::new("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-Command", command])
            .output()
            .await,
    )
}

#[cfg(not(target_os = "windows"))]
async fn execute_shell_command_inner(command: &str) -> ShellCommandResult {
    use std::env;
    use tokio::process::Command;

    let shell = env::var("SHELL")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(default_unix_command_shell);
    command_output(
        Command::new(shell)
            .arg("-lc")
            .arg(command)
            .env("PATH", default_unix_command_path())
            .output()
            .await,
    )
}

#[cfg(target_os = "macos")]
fn default_unix_command_shell() -> String {
    if Path::new("/bin/zsh").exists() {
        "/bin/zsh".to_string()
    } else {
        "/bin/sh".to_string()
    }
}

#[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
fn default_unix_command_shell() -> String {
    "/bin/sh".to_string()
}

#[cfg(target_os = "macos")]
fn default_unix_command_path() -> &'static str {
    "/opt/homebrew/sbin:/opt/homebrew/bin:/usr/local/sbin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin"
}

#[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
fn default_unix_command_path() -> &'static str {
    "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin"
}

fn command_output(result: std::io::Result<std::process::Output>) -> ShellCommandResult {
    match result {
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout).to_string();
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            let combined = if stderr.trim().is_empty() {
                stdout
            } else if stdout.trim().is_empty() {
                format!("Errors:\n{stderr}")
            } else {
                format!("{stdout}\n\nErrors:\n{stderr}")
            };
            ShellCommandResult {
                output: combined,
                exit_code: output.status.code(),
            }
        }
        Err(error) => ShellCommandResult {
            output: format!("Failed to execute command: {error}"),
            exit_code: Some(-1),
        },
    }
}

fn truncate_output(output: String) -> String {
    if output.len() <= MAX_OUTPUT_BYTES {
        return output;
    }
    truncate_utf8(&output, MAX_OUTPUT_BYTES)
}

fn truncate_utf8(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_string();
    }
    if max_bytes <= TRUNCATION_SUFFIX.len() {
        return String::new();
    }

    let mut end = max_bytes - TRUNCATION_SUFFIX.len();
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    let mut truncated = value[..end].to_string();
    truncated.push_str(TRUNCATION_SUFFIX);
    truncated
}

fn to_patch_job(job: &RemediationJob) -> patching::PatchRemediationJob {
    patching::PatchRemediationJob {
        id: job.command_id.clone(),
        organization_id: job.organization_id.clone(),
        agent_id: job.agent_id.clone(),
        intent_id: job.intent_id.clone(),
        status: "running".to_string(),
        dedupe_key: job.dedupe_key.clone(),
        metadata: job.metadata.clone(),
        requested_at: job.requested_at.clone(),
        started_at: None,
        finished_at: None,
        steps: job
            .steps
            .iter()
            .enumerate()
            .map(|(index, step)| patching::PatchRemediationStep {
                id: format!("{}:{index}", job.command_id),
                step_index: step
                    .get("stepIndex")
                    .or_else(|| step.get("step_index"))
                    .and_then(Value::as_i64)
                    .unwrap_or(index as i64) as i32,
                command: step
                    .get("command")
                    .and_then(Value::as_str)
                    .unwrap_or("talos-patch-install")
                    .to_string(),
                status: step
                    .get("status")
                    .and_then(Value::as_str)
                    .unwrap_or("pending")
                    .to_string(),
                evidence: step.get("evidence").cloned(),
                started_at: None,
                finished_at: None,
            })
            .collect(),
    }
}

fn job_summary(job: &RemediationJob) -> Value {
    json!({
        "commandId": job.command_id,
        "organizationId": job.organization_id,
        "agentId": job.agent_id,
        "intentId": job.intent_id,
        "dedupeKey": job.dedupe_key,
        "requestedBy": job.requested_by,
        "requestedAt": job.requested_at,
        "stepCount": job.steps.len()
    })
}

fn send_remediation_job_update(
    outbound_tx: &mpsc::UnboundedSender<Message>,
    command_id: &str,
    status: &str,
    step_index: i32,
    evidence: Value,
) -> Result<()> {
    send_envelope(
        outbound_tx,
        "remediation_job_update",
        RemediationJobUpdatePayload {
            command_id: command_id.to_string(),
            status: status.to_string(),
            step_index,
            evidence,
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
    .context("serialize remediation envelope")?;
    outbound_tx
        .send(Message::Text(text))
        .map_err(|_| anyhow::anyhow!("websocket outbound channel closed"))
}

#[cfg(all(test, target_os = "macos"))]
mod macos_command_tests {
    use super::*;

    #[test]
    fn macos_command_path_includes_homebrew_and_system_paths() {
        let path = default_unix_command_path();

        assert!(path.contains("/opt/homebrew/bin"));
        assert!(path.contains("/usr/local/bin"));
        assert!(path.contains("/usr/bin"));
        assert!(path.contains("/bin"));
    }

    #[test]
    fn macos_default_shell_prefers_existing_interactive_shell() {
        let shell = default_unix_command_shell();

        assert!(shell == "/bin/zsh" || shell == "/bin/sh");
        assert!(Path::new(&shell).exists());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_job(steps: Vec<Value>) -> RemediationJob {
        RemediationJob {
            command_id: "command-three-steps".to_string(),
            organization_id: "org-test".to_string(),
            agent_id: "agent-test".to_string(),
            intent_id: "generic.intent".to_string(),
            decision_id: None,
            dedupe_key: Some("dedupe-test".to_string()),
            requested_by: "test".to_string(),
            requested_at: "2026-08-17T12:00:00Z".to_string(),
            approval_state: "approved".to_string(),
            metadata: json!({}),
            steps,
            execution: json!({ "maxRetries": 0, "timeoutSeconds": 10, "stopOnFailure": true }),
        }
    }

    fn message_payload(message: Message) -> Value {
        let Message::Text(text) = message else {
            panic!("expected text remediation update");
        };
        serde_json::from_str(&text).expect("remediation update must be valid JSON")
    }

    #[tokio::test]
    async fn three_step_job_reports_each_start_and_one_bounded_atomic_outcome() {
        let (outbound_tx, mut outbound_rx) = mpsc::unbounded_channel();
        let job = test_job(vec![
            json!({ "stepIndex": 0, "command": "echo step-zero" }),
            json!({ "stepIndex": 1, "command": "echo step-one" }),
            json!({ "stepIndex": 2, "command": "echo step-two" }),
        ]);

        run_remediation_job(&outbound_tx, job)
            .await
            .expect("three-step remediation should finish");

        let updates = std::iter::from_fn(|| outbound_rx.try_recv().ok())
            .map(message_payload)
            .collect::<Vec<_>>();
        assert_eq!(updates.len(), 4);
        for (expected_index, update) in updates[..3].iter().enumerate() {
            assert_eq!(update["type"], "remediation_job_update");
            assert_eq!(update["data"]["status"], "running");
            assert_eq!(update["data"]["stepIndex"], expected_index as i32);
        }

        let final_update = &updates[3];
        assert_eq!(final_update["data"]["status"], "completed");
        assert_eq!(final_update["data"]["stepIndex"], 2);
        let evidence = &final_update["data"]["evidence"];
        assert_eq!(
            evidence["steps"]
                .as_array()
                .expect("final evidence steps")
                .iter()
                .map(|step| step["status"].as_str())
                .collect::<Vec<_>>(),
            vec![Some("completed"), Some("completed"), Some("completed")]
        );
        assert!(
            serde_json::to_vec(evidence)
                .expect("serialize evidence")
                .len()
                <= MAX_EVIDENCE_BYTES
        );
    }

    #[test]
    fn evidence_truncation_is_utf8_safe_and_respects_api_limit() {
        let job = test_job(vec![json!({ "stepIndex": 0, "command": "echo large" })]);
        let outcome = vec![StepResult {
            step_index: 0,
            command: "echo large".to_string(),
            exit_code: Some(0),
            output: "🦀".repeat(MAX_OUTPUT_BYTES),
            status: "completed",
        }];

        let evidence = build_generic_job_evidence(&job, &outcome, "completed")
            .expect("large output evidence should be bounded");
        assert!(
            serde_json::to_vec(&evidence)
                .expect("serialize evidence")
                .len()
                <= MAX_EVIDENCE_BYTES
        );
        assert!(evidence["steps"][0]["output"]
            .as_str()
            .expect("bounded output")
            .ends_with("...[truncated]"));
    }
}
