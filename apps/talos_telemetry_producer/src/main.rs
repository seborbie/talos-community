//! RMM Telemetry Producer: HTTP server that accepts telemetry from RMM Server and produces to Redpanda.

use std::env;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use anyhow::{anyhow, Context, Result};
use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::post,
    Json, Router,
};
use chrono::Utc;
use kafka::producer::{Producer, Record, RequiredAcks};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::net::TcpListener;
use tracing::{debug, error, info, warn};

const DEFAULT_PRODUCER_BIND_ADDR: &str = "127.0.0.1:17120";

#[derive(Clone)]
struct AppState {
    config: Arc<Config>,
    producer: Arc<Mutex<Producer>>,
}

#[derive(Clone)]
struct Config {
    bind_addr: String,
    server_api_key: String,
    snapshot_topic: String,
    events_topic: String,
    remediation_commands_topic: String,
    remediation_status_topic: String,
    patch_progress_topic: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SnapshotRequest {
    organization_id: String,
    agent_id: String,
    collected_at: String,
    snapshot: Value,
    #[serde(default)]
    snapshot_request_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct EventsRequest {
    organization_id: String,
    agent_id: String,
    events: Vec<Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RemediationCommandsRequest {
    commands: Vec<Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RemediationStatusRequest {
    statuses: Vec<Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PatchProgressRequest {
    progress: Vec<Value>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AcceptedResponse {
    accepted: bool,
}

fn require_server_key(headers: &HeaderMap, expected: &str) -> Result<(), (StatusCode, String)> {
    let key = headers
        .get("x-rmm-server-key")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .trim();
    if key.is_empty() {
        warn!("telemetry request unauthorized: missing x-rmm-server-key");
        return Err((
            StatusCode::UNAUTHORIZED,
            "missing x-rmm-server-key".to_string(),
        ));
    }
    if key != expected {
        warn!("telemetry request unauthorized: invalid x-rmm-server-key");
        return Err((
            StatusCode::UNAUTHORIZED,
            "invalid x-rmm-server-key".to_string(),
        ));
    }
    Ok(())
}

async fn post_snapshots(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<SnapshotRequest>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    require_server_key(&headers, &state.config.server_api_key)?;

    if body.organization_id.trim().is_empty() || body.agent_id.trim().is_empty() {
        warn!(agent_id = %body.agent_id, "snapshot rejected: empty agentId");
        return Err((
            StatusCode::BAD_REQUEST,
            "organizationId and agentId are required".to_string(),
        ));
    }
    if body.collected_at.trim().is_empty() {
        warn!(agent_id = %body.agent_id, "snapshot rejected: empty collectedAt");
        return Err((
            StatusCode::BAD_REQUEST,
            "collectedAt is required".to_string(),
        ));
    }
    if body.snapshot.is_null() || body.snapshot.is_array() {
        warn!(agent_id = %body.agent_id, "snapshot rejected: snapshot must be an object");
        return Err((
            StatusCode::BAD_REQUEST,
            "snapshot must be an object".to_string(),
        ));
    }

    let received_at = Utc::now().to_rfc3339();
    let topic = state.config.snapshot_topic.clone();
    let topic_err = topic.clone();
    let key = format!("{}:{}", body.organization_id, body.agent_id);
    let mut value = serde_json::json!({
        "organizationId": body.organization_id.clone(),
        "agentId": body.agent_id.clone(),
        "collectedAt": body.collected_at,
        "receivedAt": received_at,
        "snapshot": body.snapshot,
    });
    if let Some(ref id) = body.snapshot_request_id {
        value["snapshotRequestId"] = serde_json::Value::String(id.clone());
    }
    let value_str = value.to_string();
    let producer = Arc::clone(&state.producer);

    tokio::task::spawn_blocking(move || {
        let mut prod = match producer.lock() {
            Ok(p) => p,
            Err(e) => e.into_inner(),
        };
        prod.send(&Record::from_key_value(
            &topic,
            key.as_bytes(),
            value_str.as_bytes(),
        ))
    })
    .await
    .map_err(|e| {
        error!(error = %e, "spawn_blocking failed");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal error".to_string(),
        )
    })?
    .map_err(|e| {
        error!(
            error = %e,
            agent_id = %body.agent_id,
            topic = %topic_err,
            "kafka produce failed"
        );
        (
            StatusCode::SERVICE_UNAVAILABLE,
            "telemetry ingestion unavailable".to_string(),
        )
    })?;

    debug!(
        agent_id = %body.agent_id,
        endpoint = "snapshots",
        "snapshot accepted"
    );
    Ok((
        StatusCode::ACCEPTED,
        Json(AcceptedResponse { accepted: true }),
    ))
}

async fn post_events(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<EventsRequest>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    require_server_key(&headers, &state.config.server_api_key)?;

    if body.organization_id.trim().is_empty() || body.agent_id.trim().is_empty() {
        warn!(agent_id = %body.agent_id, "events rejected: empty agentId");
        return Err((
            StatusCode::BAD_REQUEST,
            "organizationId and agentId are required".to_string(),
        ));
    }
    if body.events.is_empty() {
        return Ok((
            StatusCode::ACCEPTED,
            Json(AcceptedResponse { accepted: true }),
        ));
    }

    let received_at = Utc::now().to_rfc3339();
    let topic = state.config.events_topic.clone();
    let topic_err = topic.clone();
    let organization_id = body.organization_id.clone();
    let agent_id = body.agent_id.clone();
    let producer = Arc::clone(&state.producer);

    let records: Vec<(String, String)> = body
        .events
        .iter()
        .map(|event| {
            let key = format!("{}:{}", organization_id, agent_id);
            let value = serde_json::json!({
                "organizationId": organization_id.clone(),
                "agentId": agent_id.clone(),
                "receivedAt": received_at.clone(),
                "event": event,
            });
            (key, value.to_string())
        })
        .collect();

    tokio::task::spawn_blocking(move || {
        let mut prod = match producer.lock() {
            Ok(p) => p,
            Err(e) => e.into_inner(),
        };
        for (key, value_str) in records {
            prod.send(&Record::from_key_value(
                &topic,
                key.as_bytes(),
                value_str.as_bytes(),
            ))?
        }
        Ok::<(), kafka::Error>(())
    })
    .await
    .map_err(|e| {
        error!(error = %e, "spawn_blocking failed");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal error".to_string(),
        )
    })?
    .map_err(|e| {
        error!(
            error = %e,
            agent_id = %agent_id,
            topic = %topic_err,
            "kafka produce failed"
        );
        (
            StatusCode::SERVICE_UNAVAILABLE,
            "telemetry ingestion unavailable".to_string(),
        )
    })?;

    debug!(
        agent_id = %body.agent_id,
        endpoint = "events",
        count = body.events.len(),
        "events accepted"
    );
    Ok((
        StatusCode::ACCEPTED,
        Json(AcceptedResponse { accepted: true }),
    ))
}

fn remediation_record_identity(value: &Value) -> Result<(String, String), (StatusCode, String)> {
    let organization_id = value
        .get("organizationId")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            (
                StatusCode::BAD_REQUEST,
                "organizationId is required".to_string(),
            )
        })?;
    let agent_id = value
        .get("agentId")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "agentId is required".to_string()))?;
    let command_id = value
        .get("commandId")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "commandId is required".to_string()))?;
    Ok((
        format!("{organization_id}:{agent_id}"),
        command_id.to_string(),
    ))
}

async fn produce_remediation_records(
    state: AppState,
    headers: HeaderMap,
    topic: String,
    records: Vec<Value>,
    context: &'static str,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    require_server_key(&headers, &state.config.server_api_key)?;

    if records.is_empty() {
        return Ok((
            StatusCode::ACCEPTED,
            Json(AcceptedResponse { accepted: true }),
        ));
    }

    let mut kafka_records = Vec::with_capacity(records.len());
    for record in records {
        let (key, command_id) = remediation_record_identity(&record)?;
        kafka_records.push((key, command_id, record.to_string()));
    }

    let topic_err = topic.clone();
    let producer = Arc::clone(&state.producer);
    let count = kafka_records.len();
    tokio::task::spawn_blocking(move || {
        let mut prod = match producer.lock() {
            Ok(p) => p,
            Err(e) => e.into_inner(),
        };
        for (key, _command_id, value_str) in kafka_records {
            prod.send(&Record::from_key_value(
                &topic,
                key.as_bytes(),
                value_str.as_bytes(),
            ))?;
        }
        Ok::<(), kafka::Error>(())
    })
    .await
    .map_err(|e| {
        error!(error = %e, "spawn_blocking failed");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal error".to_string(),
        )
    })?
    .map_err(|e| {
        error!(
            error = %e,
            topic = %topic_err,
            context,
            "kafka produce failed"
        );
        (
            StatusCode::SERVICE_UNAVAILABLE,
            "telemetry ingestion unavailable".to_string(),
        )
    })?;

    debug!(count, context, topic = %topic_err, "remediation records accepted");
    Ok((
        StatusCode::ACCEPTED,
        Json(AcceptedResponse { accepted: true }),
    ))
}

async fn post_remediation_commands(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<RemediationCommandsRequest>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let topic = state.config.remediation_commands_topic.clone();
    produce_remediation_records(state, headers, topic, body.commands, "remediation_commands").await
}

async fn post_remediation_status(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<RemediationStatusRequest>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let topic = state.config.remediation_status_topic.clone();
    produce_remediation_records(state, headers, topic, body.statuses, "remediation_status").await
}

async fn post_patch_progress(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<PatchProgressRequest>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let topic = state.config.patch_progress_topic.clone();
    produce_remediation_records(state, headers, topic, body.progress, "patch_progress").await
}

impl Config {
    fn from_env() -> Result<Self> {
        let bind_addr = env::var("RMM_TELEMETRY_PRODUCER_BIND_ADDR")
            .unwrap_or_else(|_| DEFAULT_PRODUCER_BIND_ADDR.to_string());
        let server_api_key = env::var("RMM_SERVER_API_KEY")
            .context("RMM_SERVER_API_KEY is required")?
            .trim()
            .to_string();
        if server_api_key.is_empty() {
            return Err(anyhow!("RMM_SERVER_API_KEY must not be empty"));
        }

        let kafka_brokers = env::var("RMM_TELEMETRY_KAFKA_BROKERS")
            .context("RMM_TELEMETRY_KAFKA_BROKERS is required")?;
        let kafka_brokers: Vec<String> = kafka_brokers
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        if kafka_brokers.is_empty() {
            return Err(anyhow!("RMM_TELEMETRY_KAFKA_BROKERS must not be empty"));
        }

        let snapshot_topic = env::var("RMM_TELEMETRY_SNAPSHOT_TOPIC")
            .context("RMM_TELEMETRY_SNAPSHOT_TOPIC is required")?
            .trim()
            .to_string();
        let events_topic = env::var("RMM_TELEMETRY_EVENTS_TOPIC")
            .context("RMM_TELEMETRY_EVENTS_TOPIC is required")?
            .trim()
            .to_string();
        let remediation_commands_topic = env::var("RMM_TELEMETRY_REMEDIATION_COMMANDS_TOPIC")
            .unwrap_or_else(|_| "rmm_telemetry_remediation_commands".to_string())
            .trim()
            .to_string();
        let remediation_status_topic = env::var("RMM_TELEMETRY_REMEDIATION_STATUS_TOPIC")
            .unwrap_or_else(|_| "rmm_telemetry_remediation_status".to_string())
            .trim()
            .to_string();
        let patch_progress_topic = env::var("RMM_TELEMETRY_PATCH_PROGRESS_TOPIC")
            .unwrap_or_else(|_| "rmm_telemetry_patch_progress".to_string())
            .trim()
            .to_string();

        Ok(Self {
            bind_addr,
            server_api_key,
            snapshot_topic,
            events_topic,
            remediation_commands_topic,
            remediation_status_topic,
            patch_progress_topic,
        })
    }
}

fn load_dotenv() {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut candidates = Vec::new();
    candidates.push(manifest.join("..").join(".env"));
    if let Ok(cwd) = env::current_dir() {
        candidates.push(cwd.join(".env"));
    }
    if let Ok(exe) = env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join("..").join("..").join(".env"));
        }
    }
    for path in candidates {
        if path.is_file() {
            let _ = dotenvy::from_path(&path);
            break;
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    load_dotenv();

    let log_filter =
        env::var("RUST_LOG").unwrap_or_else(|_| "info,talos_telemetry_producer=debug".to_string());
    tracing_subscriber::fmt().with_env_filter(log_filter).init();

    let config = Config::from_env()?;

    let kafka_brokers: Vec<String> = env::var("RMM_TELEMETRY_KAFKA_BROKERS")
        .unwrap()
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    let producer = Producer::from_hosts(kafka_brokers)
        .with_required_acks(RequiredAcks::One)
        .create()
        .context("create kafka producer")?;

    let state = AppState {
        config: Arc::new(config.clone()),
        producer: Arc::new(Mutex::new(producer)),
    };

    let app = Router::new()
        .route("/telemetry/snapshots", post(post_snapshots))
        .route("/telemetry/events", post(post_events))
        .route(
            "/telemetry/remediation/commands",
            post(post_remediation_commands),
        )
        .route(
            "/telemetry/remediation/status",
            post(post_remediation_status),
        )
        .route("/telemetry/patch/progress", post(post_patch_progress))
        .with_state(state);

    let bind_addr: SocketAddr = config.bind_addr.parse().context("parse bind address")?;
    info!(
        bind_addr = %config.bind_addr,
        snapshot_topic = %config.snapshot_topic,
        events_topic = %config.events_topic,
        remediation_commands_topic = %config.remediation_commands_topic,
        remediation_status_topic = %config.remediation_status_topic,
        patch_progress_topic = %config.patch_progress_topic,
        "rmm telemetry producer started"
    );
    let listener = TcpListener::bind(bind_addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_native_default_bind_is_loopback_only() {
        let address = DEFAULT_PRODUCER_BIND_ADDR
            .parse::<SocketAddr>()
            .expect("default telemetry producer address must parse");

        assert!(address.ip().is_loopback());
    }
}
