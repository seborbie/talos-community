use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

const PATCH_INSTALL_INTENT_ID: &str = "talos.patch.install";

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RemediationCommandsEnqueueRequest {
    pub(crate) commands: Vec<RemediationCommandJob>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RemediationCommandsEnqueueResponse {
    pub(crate) accepted: bool,
    pub(crate) queued: usize,
    pub(crate) connected_agents: usize,
    pub(crate) notified_agents: usize,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RemediationJobsPollPayload {
    pub(crate) request_id: String,
    pub(crate) limit: Option<u32>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RemediationJobsResponsePayload {
    pub(crate) request_id: String,
    pub(crate) jobs: Vec<RemediationCommandJob>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RemediationJobUpdatePayload {
    pub(crate) command_id: String,
    pub(crate) status: String,
    pub(crate) step_index: Option<i32>,
    pub(crate) evidence: Option<Value>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RemediationCommandJob {
    pub(crate) command_id: String,
    pub(crate) organization_id: String,
    pub(crate) agent_id: String,
    pub(crate) intent_id: String,
    #[serde(default)]
    pub(crate) decision_id: Option<String>,
    #[serde(default)]
    pub(crate) dedupe_key: Option<String>,
    pub(crate) requested_by: String,
    pub(crate) requested_at: String,
    pub(crate) approval_state: String,
    #[serde(default)]
    pub(crate) metadata: Value,
    #[serde(default)]
    pub(crate) steps: Vec<Value>,
    #[serde(default)]
    pub(crate) execution: Value,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RemediationJobsAvailablePayload {
    pub(crate) reason: String,
    pub(crate) requested_by: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ApiRemediationJobsClaimResponse {
    pub(crate) jobs: Vec<RemediationCommandJob>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ApiRemediationJobStatusResponse {
    pub(crate) updated: bool,
    pub(crate) status: String,
}

pub(crate) fn notification_targets(commands: Vec<RemediationCommandJob>) -> (usize, Vec<String>) {
    let mut commands_seen = HashSet::new();
    let mut target_agents = HashSet::new();

    for command in commands {
        if command.command_id.trim().is_empty()
            || command.organization_id.trim().is_empty()
            || command.agent_id.trim().is_empty()
            || command.intent_id.trim().is_empty()
            || command.steps.is_empty()
            || command.approval_state == "pending_approval"
            || command.intent_id == PATCH_INSTALL_INTENT_ID
        {
            continue;
        }
        if commands_seen.insert((command.agent_id.clone(), command.command_id)) {
            target_agents.insert(command.agent_id);
        }
    }

    let mut target_agents = target_agents.into_iter().collect::<Vec<_>>();
    target_agents.sort();
    (commands_seen.len(), target_agents)
}

pub(crate) fn status_request_body(payload: &RemediationJobUpdatePayload) -> Value {
    json!({
        "status": payload.status,
        "stepIndex": payload.step_index.unwrap_or(0),
        "evidence": payload.evidence,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn command(command_id: &str, agent_id: &str, intent_id: &str) -> RemediationCommandJob {
        RemediationCommandJob {
            command_id: command_id.to_string(),
            organization_id: "org-1".to_string(),
            agent_id: agent_id.to_string(),
            intent_id: intent_id.to_string(),
            decision_id: None,
            dedupe_key: None,
            requested_by: "consumer".to_string(),
            requested_at: "2026-08-17T12:00:00Z".to_string(),
            approval_state: "approved".to_string(),
            metadata: json!({}),
            steps: vec![json!({ "stepIndex": 0, "command": "echo ok" })],
            execution: json!({}),
        }
    }

    #[test]
    fn notification_targets_exclude_unapproved_patch_and_duplicate_commands() {
        let mut pending = command("pending", "agent-1", "generic.intent");
        pending.approval_state = "pending_approval".to_string();
        let patch = command("patch", "agent-1", PATCH_INSTALL_INTENT_ID);

        let (queued, agents) = notification_targets(vec![
            command("generic", "agent-2", "generic.intent"),
            command("generic", "agent-2", "generic.intent"),
            pending,
            patch,
        ]);

        assert_eq!(queued, 1);
        assert_eq!(agents, ["agent-2"]);
    }

    #[test]
    fn durable_claim_response_serializes_to_the_existing_worker_shape() {
        let claimed: ApiRemediationJobsClaimResponse = serde_json::from_value(json!({
            "jobs": [{
                "commandId": "command-1",
                "organizationId": "org-1",
                "agentId": "agent-1",
                "intentId": "generic.intent",
                "decisionId": "42",
                "dedupeKey": "dedupe-1",
                "requestedBy": "routing-engine",
                "requestedAt": "2026-08-17T12:00:00.000Z",
                "approvalState": "approved",
                "metadata": { "source": "test" },
                "steps": [{
                    "stepIndex": 0,
                    "command": "echo ok",
                    "status": "pending",
                    "timeoutSeconds": 45
                }],
                "execution": {
                    "maxRetries": 2,
                    "timeoutSeconds": 90,
                    "stopOnFailure": true
                }
            }]
        }))
        .unwrap();

        let worker_payload = serde_json::to_value(RemediationJobsResponsePayload {
            request_id: "request-1".to_string(),
            jobs: claimed.jobs,
        })
        .unwrap();

        assert_eq!(worker_payload["requestId"], "request-1");
        assert_eq!(worker_payload["jobs"][0]["commandId"], "command-1");
        assert_eq!(worker_payload["jobs"][0]["decisionId"], "42");
        assert_eq!(worker_payload["jobs"][0]["steps"][0]["timeoutSeconds"], 45);
        assert_eq!(worker_payload["jobs"][0]["execution"]["maxRetries"], 2);
    }

    #[test]
    fn status_request_uses_only_worker_report_fields() {
        let body = status_request_body(&RemediationJobUpdatePayload {
            command_id: "command-1".to_string(),
            status: "completed".to_string(),
            step_index: Some(2),
            evidence: Some(json!({ "exitCode": 0 })),
        });

        assert_eq!(
            body,
            json!({
                "status": "completed",
                "stepIndex": 2,
                "evidence": { "exitCode": 0 }
            })
        );
    }
}
