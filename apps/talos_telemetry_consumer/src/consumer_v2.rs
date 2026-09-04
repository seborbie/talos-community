use std::collections::{HashMap, HashSet};
use std::env;
use std::io::Write;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use chrono::{DateTime, Utc};
use flate2::write::GzEncoder;
use flate2::Compression;
use futures_util::StreamExt;
use hmac::{Hmac, Mac};
use kafka::producer::{Producer, Record, RequiredAcks};
use samsa::prelude::{
    fetch_offset, find_coordinator, list_offsets, BrokerAddress, BrokerConnection, ClusterMetadata,
    ConsumerGroup, ConsumerGroupBuilder, Error as KafkaConsumerError, KafkaCode, TcpConnection,
    TopicPartitions, TopicPartitionsBuilder,
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use tokio::time::sleep;
use tracing::{debug, error, info, warn, Level};
use urlencoding::encode;

type HmacSha256 = Hmac<Sha256>;

const SHARED_SERVICE_KEY_PLACEHOLDER: &str = "replace_with_shared_service_key";

#[derive(Clone)]
struct Config {
    kafka_brokers: Vec<String>,
    snapshot_topic: String,
    events_topic: String,
    remediation_commands_topic: String,
    remediation_status_topic: String,
    patch_progress_topic: String,
    dlq_topic: String,
    remediation_dlq_topic: String,
    consumer_group: String,
    consumer_session_timeout_ms: i32,
    consumer_rebalance_timeout_ms: i32,
    consumer_fetch_max_wait_ms: i32,
    consumer_fetch_min_bytes: i32,
    consumer_fetch_max_bytes: i32,
    consumer_fetch_max_partition_bytes: i32,
    consumer_restart_backoff_ms: u64,
    manifest_url: String,
    events_batch_url: String,
    graph_apply_url: String,
    decision_execute_url: String,
    remediation_command_project_url: String,
    remediation_status_project_url: String,
    patch_progress_project_url: String,
    remediation_enqueue_url: String,
    rules_url_base: String,
    processed_check_url: String,
    compat_snapshot_upsert_url: Option<String>,
    service_key: String,
    rmm_server_key: String,
    max_retries: u32,
    retry_base_ms: u64,
    baseline_stability_threshold: u32,
    offset_commit_retention_ms: i64,
    blob_endpoint: String,
    blob_container: String,
    blob_account_name: String,
    blob_account_key: String,
}

#[derive(Debug, Clone)]
struct SourceMeta {
    topic: String,
    partition: i32,
    offset: i64,
    source_ts: String,
    message_type: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct SnapshotEnvelope {
    organization_id: String,
    agent_id: String,
    collected_at: Option<String>,
    received_at: Option<String>,
    snapshot: Value,
    #[serde(default)]
    snapshot_request_id: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct EventEnvelope {
    organization_id: String,
    agent_id: String,
    #[serde(default)]
    received_at: Option<String>,
    event: Value,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct RemediationCommandEnvelope {
    #[serde(default)]
    schema_version: Option<i32>,
    #[serde(default)]
    event_type: Option<String>,
    command_id: String,
    organization_id: String,
    agent_id: String,
    intent_id: String,
    #[serde(default)]
    decision_id: Option<String>,
    #[serde(default)]
    dedupe_key: Option<String>,
    requested_by: String,
    requested_at: String,
    approval_state: String,
    #[serde(default)]
    metadata: Value,
    #[serde(default)]
    steps: Vec<Value>,
    #[serde(default)]
    execution: Value,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DlqPayload {
    topic: String,
    partition: i32,
    offset: i64,
    payload: Option<String>,
    error_kind: String,
    error_message: String,
    failed_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ErrorKind {
    Transient,
    Permanent,
}

#[derive(Debug)]
struct ProcessingError {
    kind: ErrorKind,
    message: String,
}

#[derive(Default)]
struct RuntimeStats {
    batches_seen: u64,
    empty_batches: u64,
    non_empty_batches: u64,
    slow_batches: u64,
    messages_processed: u64,
    snapshot_messages: u64,
    event_messages: u64,
    remediation_command_messages: u64,
    remediation_status_messages: u64,
    patch_progress_messages: u64,
    processing_failures: u64,
    dlq_publish_ok: u64,
    dlq_publish_fail: u64,
    idempotency_skipped: u64,
    idempotency_check_failures: u64,
    stream_restarts: u64,
    group_rebuilds: u64,
}

#[derive(Debug, Clone)]
struct FactCandidate {
    fact_key: String,
    fact_value: Value,
    stability_class: String,
    source: String,
    source_ts: String,
}

#[derive(Debug, Clone)]
struct BaselineShift {
    fact_key: String,
    current_value: Value,
    current_value_text: String,
    previous_value: Value,
    previous_value_text: Option<String>,
    support_ratio: Option<f64>,
    confidence_score: Option<f64>,
    scope_type: String,
}

#[derive(Debug, Clone)]
struct NormalizedEvent {
    event_id: String,
    occurred_at: DateTime<Utc>,
    received_at: DateTime<Utc>,
    event_type: String,
    severity: String,
    source: String,
    service_name: Option<String>,
    process_name: Option<String>,
    code: Option<String>,
    message: Option<String>,
    attributes: Value,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RulesResponse {
    rules: Vec<RoutingRule>,
    current_facts: Vec<CurrentFact>,
    baselines: Vec<FactBaseline>,
    #[serde(default)]
    recent_decisions: Vec<RecentDecision>,
    #[serde(default)]
    scope_baselines: Vec<ScopeBaseline>,
    #[serde(default)]
    stability_overrides: Vec<StabilityOverride>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct RoutingRule {
    id: String,
    trigger_domain: String,
    trigger_key: String,
    match_operator: String,
    match_value: Option<String>,
    #[serde(default)]
    previous_match_operator: Option<String>,
    #[serde(default)]
    previous_match_value: Option<String>,
    #[serde(default)]
    min_support_ratio: Option<f64>,
    #[serde(default)]
    min_confidence_score: Option<f64>,
    #[serde(default)]
    scope_type_filter: Option<String>,
    action: String,
    intent_id: Option<String>,
    #[serde(default)]
    cooldown_seconds: i32,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct RecentDecision {
    rule_id: String,
    decided_at: String,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
struct ScopeBaseline {
    fact_key: String,
    promoted_value: Value,
    scope_type: String,
    #[serde(default)]
    support_ratio: f64,
    #[serde(default)]
    sample_size: i32,
    confidence_score: f64,
    is_stable: bool,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct StabilityOverride {
    fact_key_pattern: String,
    stability_class: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CurrentFact {
    fact_key: String,
    fact_value: Value,
    #[serde(default)]
    stability_class: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FactBaseline {
    fact_key: String,
    promoted_value: Value,
    candidate_value: Value,
    candidate_count: i32,
    window_count: i32,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ManifestRequest {
    organization_id: String,
    agent_id: String,
    collected_at: String,
    received_at: String,
    blob_container: String,
    blob_name: String,
    blob_content_encoding: String,
    blob_size_bytes: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    snapshot_request_id: Option<String>,
    status: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct EventsBatchRequest {
    organization_id: String,
    agent_id: String,
    events: Vec<EventBatchItem>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct EventBatchItem {
    event_id: String,
    occurred_at: String,
    received_at: String,
    event_type: String,
    severity: String,
    source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    service_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    process_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
    attributes: Value,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GraphApplyRequest {
    organization_id: String,
    agent_id: String,
    source: GraphSource,
    facts: Vec<GraphFact>,
    changes: Vec<GraphChange>,
    baselines: Vec<GraphBaselineWrite>,
    decision: GraphDecision,
    idempotency_key: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GraphSource {
    topic: String,
    partition: i32,
    offset: i64,
    ts: String,
    message_type: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GraphFact {
    fact_key: String,
    fact_value: Value,
    stability_class: String,
    source: String,
    source_ts: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GraphChange {
    fact_key: String,
    previous_value: Value,
    next_value: Value,
    change_kind: String,
    source: String,
    source_ts: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GraphBaselineWrite {
    fact_key: String,
    promoted_value: Value,
    candidate_value: Value,
    candidate_count: i32,
    window_count: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_changed_at: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GraphDecision {
    domain: String,
    trigger_key: String,
    trigger_value: Value,
    action: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    matched_rule_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    intent_id: Option<String>,
    reason: String,
    dedupe_key: String,
    source: String,
    source_ts: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AcceptedResponse {
    accepted: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProcessedCheckResponse {
    accepted: bool,
    processed: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GraphApplyResponse {
    accepted: bool,
    duplicate: Option<bool>,
    decision_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RemediationProjectResponse {
    accepted: bool,
    status: String,
}

#[derive(Debug, Clone)]
struct DecisionOutcome {
    domain: String,
    trigger_key: String,
    trigger_value: Value,
    action: String,
    matched_rule_id: Option<String>,
    intent_id: Option<String>,
    reason: String,
    dedupe_key: String,
}

#[derive(Debug, Clone)]
struct RoutingCandidate {
    domain: String,
    trigger_key: String,
    current_value: Value,
    current_value_text: String,
    previous_value: Value,
    previous_value_text: Option<String>,
    support_ratio: Option<f64>,
    confidence_score: Option<f64>,
    scope_type: String,
}

#[derive(Debug)]
struct GraphBuildResult {
    request: GraphApplyRequest,
    #[allow(dead_code)]
    decision: DecisionOutcome,
}

fn load_dotenv() {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut candidates = vec![manifest.join("..").join(".env")];
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
    let log_filter = env::var("RUST_LOG")
        .unwrap_or_else(|_| "info,samsa=warn,talos_telemetry_consumer=debug".to_string());
    tracing_subscriber::fmt().with_env_filter(log_filter).init();

    let cfg = Config::from_env()?;
    info!(
        brokers = ?cfg.kafka_brokers,
        snapshot_topic = %cfg.snapshot_topic,
        events_topic = %cfg.events_topic,
        remediation_commands_topic = %cfg.remediation_commands_topic,
        remediation_status_topic = %cfg.remediation_status_topic,
        patch_progress_topic = %cfg.patch_progress_topic,
        dlq_topic = %cfg.dlq_topic,
        remediation_dlq_topic = %cfg.remediation_dlq_topic,
        consumer_group = %cfg.consumer_group,
        consumer_session_timeout_ms = cfg.consumer_session_timeout_ms,
        consumer_rebalance_timeout_ms = cfg.consumer_rebalance_timeout_ms,
        consumer_fetch_max_wait_ms = cfg.consumer_fetch_max_wait_ms,
        consumer_fetch_min_bytes = cfg.consumer_fetch_min_bytes,
        consumer_fetch_max_bytes = cfg.consumer_fetch_max_bytes,
        consumer_fetch_max_partition_bytes = cfg.consumer_fetch_max_partition_bytes,
        consumer_restart_backoff_ms = cfg.consumer_restart_backoff_ms,
        manifest_url = %cfg.manifest_url,
        events_batch_url = %cfg.events_batch_url,
        graph_apply_url = %cfg.graph_apply_url,
        decision_execute_url = %cfg.decision_execute_url,
        remediation_command_project_url = %cfg.remediation_command_project_url,
        remediation_status_project_url = %cfg.remediation_status_project_url,
        patch_progress_project_url = %cfg.patch_progress_project_url,
        remediation_enqueue_url = %cfg.remediation_enqueue_url,
        rules_url_base = %cfg.rules_url_base,
        processed_check_url = %cfg.processed_check_url,
        compat_snapshot_upsert_url = ?cfg.compat_snapshot_upsert_url,
        max_retries = cfg.max_retries,
        retry_base_ms = cfg.retry_base_ms,
        baseline_stability_threshold = cfg.baseline_stability_threshold,
        offset_commit_retention_ms = cfg.offset_commit_retention_ms,
        "telemetry consumer configuration loaded"
    );

    let http = reqwest::Client::new();
    ensure_blob_container(&cfg, &http)
        .await
        .map_err(|e| anyhow!("{}", e.message))?;

    let mut dlq_producer = Producer::from_hosts(cfg.kafka_brokers.clone())
        .with_required_acks(RequiredAcks::One)
        .create()
        .context("create dlq producer")?;

    let brokers = parse_brokers(&cfg.kafka_brokers)?;
    let configured_topics = configured_consumer_topics(&cfg);
    let assignment = discover_topic_assignment(&brokers, &configured_topics).await?;
    info!(topic_partitions = ?assignment, "discovered telemetry topic partitions");
    validate_group_offsets_preflight(&brokers, &cfg.consumer_group, &assignment).await?;
    let mut stats = RuntimeStats::default();
    let mut last_summary_at = Instant::now();
    let mut stream_generation = 0_u64;

    info!("rmm telemetry consumer started");
    'consume: loop {
        let member = match build_consumer_group_member(&cfg, &brokers, &assignment).await {
            Ok(member) => {
                stream_generation += 1;
                stats.group_rebuilds += 1;
                info!(stream_generation, "consumer group member ready");
                member
            }
            Err(error) => {
                warn!(
                    %error,
                    backoff_ms = cfg.consumer_restart_backoff_ms,
                    "failed to build consumer group member; retrying"
                );
                maybe_log_runtime_summary(&stats, &mut last_summary_at);
                sleep(Duration::from_millis(cfg.consumer_restart_backoff_ms)).await;
                continue;
            }
        };

        let stream = member.into_stream();
        tokio::pin!(stream);

        loop {
            match stream.next().await {
                Some(Ok(batch)) => {
                    stats.batches_seen += 1;
                    let batch_started_at = Instant::now();
                    let mut batch_messages = 0usize;
                    for msg in batch {
                        batch_messages += 1;
                        stats.messages_processed += 1;
                        let payload = msg.value.to_vec();
                        let source = SourceMeta {
                            topic: msg.topic_name.clone(),
                            partition: msg.partition_index,
                            offset: msg.offset as i64,
                            source_ts: Utc::now().to_rfc3339(),
                            message_type: if msg.topic_name == cfg.snapshot_topic {
                                stats.snapshot_messages += 1;
                                "snapshot".to_string()
                            } else if msg.topic_name == cfg.events_topic {
                                stats.event_messages += 1;
                                "event".to_string()
                            } else if msg.topic_name == cfg.remediation_commands_topic {
                                stats.remediation_command_messages += 1;
                                "remediation_command".to_string()
                            } else if msg.topic_name == cfg.remediation_status_topic {
                                stats.remediation_status_messages += 1;
                                "remediation_status".to_string()
                            } else if msg.topic_name == cfg.patch_progress_topic {
                                stats.patch_progress_messages += 1;
                                "patch_progress".to_string()
                            } else {
                                "unknown".to_string()
                            },
                        };
                        match is_message_already_processed(&cfg, &http, &source).await {
                            Ok(true) => {
                                stats.idempotency_skipped += 1;
                                debug!(
                                    topic = %source.topic,
                                    partition = source.partition,
                                    offset = source.offset,
                                    "skipping already-processed message per api state"
                                );
                                continue;
                            }
                            Ok(false) => {}
                            Err(error) => {
                                stats.idempotency_check_failures += 1;
                                warn!(
                                    kind = ?error.kind,
                                    error = %error.message,
                                    topic = %source.topic,
                                    partition = source.partition,
                                    offset = source.offset,
                                    "processed-state check failed; proceeding with processing"
                                );
                            }
                        }

                        debug!(
                            topic = %source.topic,
                            partition = source.partition,
                            offset = source.offset,
                            payload_bytes = payload.len(),
                            message_type = %source.message_type,
                            "processing consumed message"
                        );

                        if let Err(err) = process_with_retries(&cfg, &http, &payload, &source).await
                        {
                            stats.processing_failures += 1;
                            error!(kind=?err.kind, error=%err.message, "message failed, sending to dlq");
                            let dlq = DlqPayload {
                                topic: source.topic.clone(),
                                partition: source.partition,
                                offset: source.offset,
                                payload: Some(String::from_utf8_lossy(&payload).to_string()),
                                error_kind: match err.kind {
                                    ErrorKind::Transient => "transient".into(),
                                    ErrorKind::Permanent => "permanent".into(),
                                },
                                error_message: err.message,
                                failed_at: Utc::now().to_rfc3339(),
                            };
                            let target_dlq_topic = if source.message_type.starts_with("remediation")
                            {
                                cfg.remediation_dlq_topic.as_str()
                            } else {
                                cfg.dlq_topic.as_str()
                            };
                            match serde_json::to_vec(&dlq) {
                                Ok(bytes) => {
                                    if let Err(error) = dlq_producer
                                        .send(&Record::from_value(target_dlq_topic, bytes))
                                    {
                                        stats.dlq_publish_fail += 1;
                                        warn!(%error, topic = %target_dlq_topic, "failed to publish dlq; continuing");
                                    } else {
                                        stats.dlq_publish_ok += 1;
                                        debug!(
                                            topic = %target_dlq_topic,
                                            original_partition = dlq.partition,
                                            original_offset = dlq.offset,
                                            "dlq record published"
                                        );
                                    }
                                }
                                Err(error) => {
                                    stats.dlq_publish_fail += 1;
                                    warn!(%error, "failed to serialize dlq payload; continuing");
                                }
                            }
                        }
                    }

                    let batch_elapsed = batch_started_at.elapsed();
                    if batch_messages > 0 {
                        stats.non_empty_batches += 1;
                        debug!(
                            batch_messages,
                            batch_processing_ms = batch_elapsed.as_millis() as u64,
                            "consumer batch processing complete"
                        );
                        if batch_elapsed
                            >= batch_slow_warning_threshold(cfg.consumer_session_timeout_ms)
                        {
                            stats.slow_batches += 1;
                            warn!(
                                batch_messages,
                                batch_processing_ms = batch_elapsed.as_millis() as u64,
                                session_timeout_ms = cfg.consumer_session_timeout_ms,
                                "consumer batch processing time is approaching Kafka session timeout"
                            );
                        }
                    } else {
                        stats.empty_batches += 1;
                    }
                }
                Some(Err(error)) => {
                    stats.stream_restarts += 1;
                    warn!(
                        %error,
                        membership_related = is_membership_related_error(&error),
                        stream_generation,
                        backoff_ms = cfg.consumer_restart_backoff_ms,
                        "consumer stream error; rebuilding consumer group stream"
                    );
                    maybe_log_runtime_summary(&stats, &mut last_summary_at);
                    sleep(Duration::from_millis(cfg.consumer_restart_backoff_ms)).await;
                    continue 'consume;
                }
                None => {
                    stats.stream_restarts += 1;
                    warn!(
                        stream_generation,
                        backoff_ms = cfg.consumer_restart_backoff_ms,
                        "consumer stream ended unexpectedly; rebuilding consumer group stream"
                    );
                    maybe_log_runtime_summary(&stats, &mut last_summary_at);
                    sleep(Duration::from_millis(cfg.consumer_restart_backoff_ms)).await;
                    continue 'consume;
                }
            }

            maybe_log_runtime_summary(&stats, &mut last_summary_at);
        }
    }
}

async fn build_consumer_group_member(
    cfg: &Config,
    brokers: &[BrokerAddress],
    assignment: &TopicPartitions,
) -> Result<ConsumerGroup<TcpConnection>> {
    ConsumerGroupBuilder::<TcpConnection>::new(
        brokers.to_vec(),
        cfg.consumer_group.clone(),
        assignment.clone(),
    )
    .await
    .map_err(|e| anyhow!("create consumer group builder: {e:?}"))?
    .retention_time_ms(cfg.offset_commit_retention_ms)
    .session_timeout_ms(cfg.consumer_session_timeout_ms)
    .rebalance_timeout_ms(cfg.consumer_rebalance_timeout_ms)
    .max_wait_ms(cfg.consumer_fetch_max_wait_ms)
    .min_bytes(cfg.consumer_fetch_min_bytes)
    .max_bytes(cfg.consumer_fetch_max_bytes)
    .max_partition_bytes(cfg.consumer_fetch_max_partition_bytes)
    .build()
    .await
    .map_err(|e| anyhow!("build consumer group member: {e:?}"))
}

fn maybe_log_runtime_summary(stats: &RuntimeStats, last_summary_at: &mut Instant) {
    if last_summary_at.elapsed() >= Duration::from_secs(30) && tracing::enabled!(Level::INFO) {
        info!(
            batches_seen = stats.batches_seen,
            empty_batches = stats.empty_batches,
            non_empty_batches = stats.non_empty_batches,
            slow_batches = stats.slow_batches,
            messages_processed = stats.messages_processed,
            snapshot_messages = stats.snapshot_messages,
            event_messages = stats.event_messages,
            remediation_command_messages = stats.remediation_command_messages,
            remediation_status_messages = stats.remediation_status_messages,
            patch_progress_messages = stats.patch_progress_messages,
            processing_failures = stats.processing_failures,
            dlq_publish_ok = stats.dlq_publish_ok,
            dlq_publish_fail = stats.dlq_publish_fail,
            idempotency_skipped = stats.idempotency_skipped,
            idempotency_check_failures = stats.idempotency_check_failures,
            stream_restarts = stats.stream_restarts,
            group_rebuilds = stats.group_rebuilds,
            "telemetry consumer periodic summary"
        );
        *last_summary_at = Instant::now();
    }
}

fn batch_slow_warning_threshold(session_timeout_ms: i32) -> Duration {
    let timeout_ms = session_timeout_ms.max(1) as u64;
    Duration::from_millis((timeout_ms.saturating_mul(2)).max(3) / 3)
}

fn is_membership_related_error(error: &KafkaConsumerError) -> bool {
    match error {
        KafkaConsumerError::KafkaError(code) => matches!(
            code,
            KafkaCode::UnknownMemberId
                | KafkaCode::IllegalGeneration
                | KafkaCode::RebalanceInProgress
                | KafkaCode::NotCoordinatorForGroup
                | KafkaCode::GroupCoordinatorNotAvailable
                | KafkaCode::GroupLoadInProgress
        ),
        KafkaConsumerError::IoError(_)
        | KafkaConsumerError::MissingData(_)
        | KafkaConsumerError::NoConnectionForBroker(_)
        | KafkaConsumerError::MetadataNeedsSync => true,
        _ => false,
    }
}

async fn process_with_retries(
    cfg: &Config,
    http: &reqwest::Client,
    payload: &[u8],
    source: &SourceMeta,
) -> Result<(), ProcessingError> {
    let mut attempt = 0_u32;
    loop {
        attempt += 1;
        debug!(attempt, max_retries = cfg.max_retries, topic = %source.topic, "processing attempt started");
        match process_once(cfg, http, payload, source).await {
            Ok(()) => {
                debug!(attempt, topic = %source.topic, "processing attempt succeeded");
                return Ok(());
            }
            Err(err) if err.kind == ErrorKind::Permanent => return Err(err),
            Err(err) => {
                if attempt > cfg.max_retries {
                    return Err(err);
                }
                let backoff = cfg
                    .retry_base_ms
                    .saturating_mul(2_u64.saturating_pow(attempt - 1));
                warn!(attempt, backoff_ms=backoff, topic=%source.topic, "retrying transient error");
                sleep(Duration::from_millis(backoff)).await;
            }
        }
    }
}

async fn process_once(
    cfg: &Config,
    http: &reqwest::Client,
    payload: &[u8],
    source: &SourceMeta,
) -> Result<(), ProcessingError> {
    if source.topic == cfg.snapshot_topic {
        process_snapshot_message(cfg, http, payload, source).await
    } else if source.topic == cfg.events_topic {
        process_event_message(cfg, http, payload, source).await
    } else if source.topic == cfg.remediation_commands_topic {
        process_remediation_command_message(cfg, http, payload).await
    } else if source.topic == cfg.remediation_status_topic {
        process_remediation_status_message(cfg, http, payload).await
    } else if source.topic == cfg.patch_progress_topic {
        process_patch_progress_message(cfg, http, payload).await
    } else {
        Err(ProcessingError {
            kind: ErrorKind::Permanent,
            message: format!("unknown topic {}", source.topic),
        })
    }
}

async fn process_remediation_command_message(
    cfg: &Config,
    http: &reqwest::Client,
    payload: &[u8],
) -> Result<(), ProcessingError> {
    let envelope: RemediationCommandEnvelope =
        serde_json::from_slice(payload).map_err(|e| ProcessingError {
            kind: ErrorKind::Permanent,
            message: format!("invalid remediation command payload: {e}"),
        })?;
    if envelope.command_id.trim().is_empty()
        || envelope.organization_id.trim().is_empty()
        || envelope.agent_id.trim().is_empty()
        || envelope.intent_id.trim().is_empty()
    {
        return Err(ProcessingError {
            kind: ErrorKind::Permanent,
            message: "remediation command missing commandId, organizationId, agentId or intentId"
                .into(),
        });
    }
    if envelope.steps.is_empty() {
        return Err(ProcessingError {
            kind: ErrorKind::Permanent,
            message: "remediation command must include at least one step".into(),
        });
    }

    let projection = post_remediation_command_projection(cfg, http, &envelope).await?;

    if envelope.approval_state == "pending_approval" || projection.status != "queued" {
        debug!(
            command_id = %envelope.command_id,
            agent_id = %envelope.agent_id,
            status = %projection.status,
            "remediation command projected but not dispatched"
        );
        return Ok(());
    }

    post_json_with_rmm_server_key(
        cfg,
        http,
        &cfg.remediation_enqueue_url,
        &serde_json::json!({ "commands": [envelope] }),
        "remediation command enqueue",
    )
    .await
}

async fn process_remediation_status_message(
    cfg: &Config,
    http: &reqwest::Client,
    payload: &[u8],
) -> Result<(), ProcessingError> {
    let value: Value = serde_json::from_slice(payload).map_err(|e| ProcessingError {
        kind: ErrorKind::Permanent,
        message: format!("invalid remediation status payload: {e}"),
    })?;
    let command_id = value
        .get("commandId")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string();
    let organization_id = value
        .get("organizationId")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string();
    let agent_id = value
        .get("agentId")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string();
    let status = value
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string();
    if command_id.is_empty()
        || organization_id.is_empty()
        || agent_id.is_empty()
        || status.is_empty()
    {
        return Err(ProcessingError {
            kind: ErrorKind::Permanent,
            message: "remediation status missing commandId, organizationId, agentId or status"
                .into(),
        });
    }

    let compacted = compact_status_for_projection(value);
    post_json_with_service_key(
        cfg,
        http,
        &cfg.remediation_status_project_url,
        &serde_json::json!({ "statuses": [compacted] }),
        "remediation status projection",
    )
    .await
}

fn compact_status_for_projection(mut value: Value) -> Value {
    const MAX_EVIDENCE_BYTES: usize = 32 * 1024;
    if let Some(evidence) = value.get("evidence") {
        let evidence_bytes = serde_json::to_vec(evidence)
            .map(|bytes| bytes.len())
            .unwrap_or(0);
        if evidence_bytes > MAX_EVIDENCE_BYTES {
            value["evidence"] = serde_json::json!({
                "truncated": true,
                "originalBytes": evidence_bytes,
                "message": "Full remediation evidence is retained in Redpanda status events."
            });
        }
    }
    value
}

async fn process_patch_progress_message(
    cfg: &Config,
    http: &reqwest::Client,
    payload: &[u8],
) -> Result<(), ProcessingError> {
    let value = parse_patch_progress(payload)?;
    post_json_with_rmm_server_key(
        cfg,
        http,
        &cfg.patch_progress_project_url,
        &value,
        "patch progress projection",
    )
    .await
}

fn parse_patch_progress(payload: &[u8]) -> Result<Value, ProcessingError> {
    let value: Value = serde_json::from_slice(payload).map_err(|error| ProcessingError {
        kind: ErrorKind::Permanent,
        message: format!("invalid patch progress payload: {error}"),
    })?;
    let organization_id = value
        .get("organizationId")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string();
    let agent_id = value
        .get("agentId")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string();
    let job_id = value
        .get("jobId")
        .or_else(|| value.get("commandId"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string();
    if organization_id.is_empty() || agent_id.is_empty() || job_id.is_empty() {
        return Err(ProcessingError {
            kind: ErrorKind::Permanent,
            message: "patch progress missing organizationId, agentId or jobId".into(),
        });
    }
    Ok(value)
}

async fn process_snapshot_message(
    cfg: &Config,
    http: &reqwest::Client,
    payload: &[u8],
    source: &SourceMeta,
) -> Result<(), ProcessingError> {
    let envelope: SnapshotEnvelope =
        serde_json::from_slice(payload).map_err(|e| ProcessingError {
            kind: ErrorKind::Permanent,
            message: format!("invalid snapshot payload: {e}"),
        })?;
    if envelope.organization_id.trim().is_empty() || envelope.agent_id.trim().is_empty() {
        return Err(ProcessingError {
            kind: ErrorKind::Permanent,
            message: "snapshot payload missing organizationId or agentId".into(),
        });
    }

    let collected = resolve_collected_at(&envelope)?;
    let received = resolve_received_at(&envelope)?;
    let blob_name = format!(
        "snapshots/{}/{}/{}.json.gz",
        sanitize(&envelope.agent_id),
        collected.format("%Y/%m/%d"),
        collected.format("%Y%m%dT%H%M%S%.3fZ")
    );

    let snapshot_bytes = serde_json::to_vec(&envelope.snapshot).map_err(|e| ProcessingError {
        kind: ErrorKind::Permanent,
        message: format!("serialize snapshot failed: {e}"),
    })?;
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder
        .write_all(&snapshot_bytes)
        .map_err(|e| ProcessingError {
            kind: ErrorKind::Permanent,
            message: format!("gzip encode failed: {e}"),
        })?;
    let compressed = encoder.finish().map_err(|e| ProcessingError {
        kind: ErrorKind::Permanent,
        message: format!("gzip finish failed: {e}"),
    })?;
    upload_blob(cfg, http, &blob_name, &compressed).await?;

    let manifest = ManifestRequest {
        organization_id: envelope.organization_id.clone(),
        agent_id: envelope.agent_id.clone(),
        collected_at: collected.to_rfc3339(),
        received_at: received.to_rfc3339(),
        blob_container: cfg.blob_container.clone(),
        blob_name,
        blob_content_encoding: "gzip".to_string(),
        blob_size_bytes: compressed.len() as u64,
        snapshot_request_id: envelope.snapshot_request_id.clone(),
        status: "completed".to_string(),
    };
    post_manifest(cfg, http, &manifest).await?;

    if let Err(err) = post_compat_snapshot_upsert(
        cfg,
        http,
        &envelope,
        &collected,
        &received,
        &manifest.blob_name,
        compressed.len() as u64,
    )
    .await
    {
        warn!(
            kind = ?err.kind,
            error = %err.message,
            "compat snapshot upsert failed; continuing with canonical pipeline"
        );
    }

    let rules_state =
        fetch_rules_state(cfg, http, &envelope.organization_id, &envelope.agent_id).await?;
    let snapshot_facts = facts_from_snapshot(&envelope.snapshot, source);
    let graph_payload = build_graph_payload(
        &envelope.organization_id,
        &envelope.agent_id,
        source,
        &snapshot_facts,
        None,
        &rules_state,
        cfg.baseline_stability_threshold,
    );
    let graph_response = post_graph_apply(cfg, http, &graph_payload.request).await?;
    debug!(
        duplicate = graph_response.duplicate.unwrap_or(false),
        decision_id = graph_response.decision_id.as_deref().unwrap_or("none"),
        "graph apply response received for snapshot"
    );

    maybe_execute_decision(cfg, http, graph_response.decision_id).await?;
    Ok(())
}

async fn process_event_message(
    cfg: &Config,
    http: &reqwest::Client,
    payload: &[u8],
    source: &SourceMeta,
) -> Result<(), ProcessingError> {
    let envelope: EventEnvelope = serde_json::from_slice(payload).map_err(|e| ProcessingError {
        kind: ErrorKind::Permanent,
        message: format!("invalid event payload: {e}"),
    })?;
    if envelope.organization_id.trim().is_empty() || envelope.agent_id.trim().is_empty() {
        return Err(ProcessingError {
            kind: ErrorKind::Permanent,
            message: "event payload missing organizationId or agentId".into(),
        });
    }

    let normalized_event = normalize_event(&envelope, source)?;
    let events_batch = EventsBatchRequest {
        organization_id: envelope.organization_id.clone(),
        agent_id: envelope.agent_id.clone(),
        events: vec![EventBatchItem {
            event_id: normalized_event.event_id.clone(),
            occurred_at: normalized_event.occurred_at.to_rfc3339(),
            received_at: normalized_event.received_at.to_rfc3339(),
            event_type: normalized_event.event_type.clone(),
            severity: normalized_event.severity.clone(),
            source: normalized_event.source.clone(),
            service_name: normalized_event.service_name.clone(),
            process_name: normalized_event.process_name.clone(),
            code: normalized_event.code.clone(),
            message: normalized_event.message.clone(),
            attributes: normalized_event.attributes.clone(),
        }],
    };
    post_events_batch(cfg, http, &events_batch).await?;

    let rules_state =
        fetch_rules_state(cfg, http, &envelope.organization_id, &envelope.agent_id).await?;
    let event_facts = facts_from_event(&normalized_event, source);
    let graph_payload = build_graph_payload(
        &envelope.organization_id,
        &envelope.agent_id,
        source,
        &event_facts,
        Some(&normalized_event),
        &rules_state,
        cfg.baseline_stability_threshold,
    );
    let graph_response = post_graph_apply(cfg, http, &graph_payload.request).await?;
    debug!(
        duplicate = graph_response.duplicate.unwrap_or(false),
        decision_id = graph_response.decision_id.as_deref().unwrap_or("none"),
        "graph apply response received for event"
    );

    maybe_execute_decision(cfg, http, graph_response.decision_id).await?;
    Ok(())
}

async fn post_manifest(
    cfg: &Config,
    http: &reqwest::Client,
    payload: &ManifestRequest,
) -> Result<(), ProcessingError> {
    let response = http
        .post(&cfg.manifest_url)
        .header("x-service-key", &cfg.service_key)
        .json(payload)
        .send()
        .await
        .map_err(|e| ProcessingError {
            kind: ErrorKind::Transient,
            message: format!("manifest request failed: {e}"),
        })?;
    handle_accepted_response(response, "manifest").await
}

async fn post_events_batch(
    cfg: &Config,
    http: &reqwest::Client,
    payload: &EventsBatchRequest,
) -> Result<(), ProcessingError> {
    let response = http
        .post(&cfg.events_batch_url)
        .header("x-service-key", &cfg.service_key)
        .json(payload)
        .send()
        .await
        .map_err(|e| ProcessingError {
            kind: ErrorKind::Transient,
            message: format!("events batch request failed: {e}"),
        })?;
    handle_accepted_response(response, "events batch").await
}

async fn post_compat_snapshot_upsert(
    cfg: &Config,
    http: &reqwest::Client,
    envelope: &SnapshotEnvelope,
    collected: &DateTime<Utc>,
    received: &DateTime<Utc>,
    blob_name: &str,
    blob_size_bytes: u64,
) -> Result<(), ProcessingError> {
    let Some(url) = cfg.compat_snapshot_upsert_url.as_ref() else {
        return Ok(());
    };

    let mut body = serde_json::json!({
        "organizationId": envelope.organization_id,
        "agentId": envelope.agent_id,
        "collectedAt": collected.to_rfc3339(),
        "receivedAt": received.to_rfc3339(),
        "snapshot": envelope.snapshot,
        "blobContainer": cfg.blob_container,
        "blobName": blob_name,
        "blobContentEncoding": "gzip",
        "blobSizeBytes": blob_size_bytes
    });
    if let Some(ref id) = envelope.snapshot_request_id {
        body["snapshotRequestId"] = serde_json::Value::String(id.clone());
    }

    let response = http
        .post(url)
        .header("x-service-key", &cfg.service_key)
        .json(&body)
        .send()
        .await
        .map_err(|e| ProcessingError {
            kind: ErrorKind::Transient,
            message: format!("compat snapshot upsert request failed: {e}"),
        })?;
    let status = response.status();
    let response_body = response.text().await.unwrap_or_default();
    debug!(
        status = %status,
        compat_url = %url,
        body = %response_body,
        "compat snapshot upsert response received"
    );
    if status.is_server_error() {
        return Err(ProcessingError {
            kind: ErrorKind::Transient,
            message: format!("compat snapshot upsert 5xx status: {}", status),
        });
    }
    if !status.is_success() {
        return Err(ProcessingError {
            kind: ErrorKind::Permanent,
            message: format!("compat snapshot upsert non-success status: {}", status),
        });
    }
    Ok(())
}

async fn is_message_already_processed(
    cfg: &Config,
    http: &reqwest::Client,
    source: &SourceMeta,
) -> Result<bool, ProcessingError> {
    let response = http
        .post(&cfg.processed_check_url)
        .header("x-service-key", &cfg.service_key)
        .json(&serde_json::json!({
            "source": {
                "topic": source.topic,
                "partition": source.partition,
                "offset": source.offset
            }
        }))
        .send()
        .await
        .map_err(|e| ProcessingError {
            kind: ErrorKind::Transient,
            message: format!("processed-state check request failed: {e}"),
        })?;

    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(ProcessingError {
            kind: classify_http_status(status),
            message: format!("processed-state check non-success status {status}: {body}"),
        });
    }

    let parsed =
        serde_json::from_str::<ProcessedCheckResponse>(&body).map_err(|e| ProcessingError {
            kind: ErrorKind::Permanent,
            message: format!("parse processed-state check response failed: {e}"),
        })?;
    if !parsed.accepted {
        return Err(ProcessingError {
            kind: ErrorKind::Permanent,
            message: "processed-state check response not accepted".to_string(),
        });
    }

    Ok(parsed.processed)
}

async fn fetch_rules_state(
    cfg: &Config,
    http: &reqwest::Client,
    organization_id: &str,
    agent_id: &str,
) -> Result<RulesResponse, ProcessingError> {
    let url = format!(
        "{}/{}?organizationId={}",
        cfg.rules_url_base.trim_end_matches('/'),
        encode(agent_id),
        encode(organization_id)
    );
    let response = http
        .get(&url)
        .header("x-service-key", &cfg.service_key)
        .send()
        .await
        .map_err(|e| ProcessingError {
            kind: ErrorKind::Transient,
            message: format!("fetch rules request failed: {e}"),
        })?;

    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(ProcessingError {
            kind: classify_http_status(status),
            message: format!("fetch rules non-success status {status}: {body}"),
        });
    }

    serde_json::from_str::<RulesResponse>(&body).map_err(|e| ProcessingError {
        kind: ErrorKind::Permanent,
        message: format!("parse rules response failed: {e}"),
    })
}

async fn post_graph_apply(
    cfg: &Config,
    http: &reqwest::Client,
    payload: &GraphApplyRequest,
) -> Result<GraphApplyResponse, ProcessingError> {
    let response = http
        .post(&cfg.graph_apply_url)
        .header("x-service-key", &cfg.service_key)
        .json(payload)
        .send()
        .await
        .map_err(|e| ProcessingError {
            kind: ErrorKind::Transient,
            message: format!("graph apply request failed: {e}"),
        })?;

    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(ProcessingError {
            kind: classify_http_status(status),
            message: format!("graph apply non-success status {status}: {body}"),
        });
    }
    let parsed =
        serde_json::from_str::<GraphApplyResponse>(&body).map_err(|e| ProcessingError {
            kind: ErrorKind::Permanent,
            message: format!("parse graph apply response failed: {e}"),
        })?;
    if !parsed.accepted {
        return Err(ProcessingError {
            kind: ErrorKind::Permanent,
            message: "graph apply response not accepted".to_string(),
        });
    }
    Ok(parsed)
}

async fn maybe_execute_decision(
    cfg: &Config,
    http: &reqwest::Client,
    decision_id: Option<String>,
) -> Result<(), ProcessingError> {
    let Some(decision_id) = decision_id else {
        return Ok(());
    };

    let response = http
        .post(&cfg.decision_execute_url)
        .header("x-service-key", &cfg.service_key)
        .json(&serde_json::json!({ "decisionId": decision_id }))
        .send()
        .await
        .map_err(|e| ProcessingError {
            kind: ErrorKind::Transient,
            message: format!("decision execute request failed: {e}"),
        })?;
    handle_accepted_response(response, "decision execution").await
}

async fn handle_accepted_response(
    response: reqwest::Response,
    context: &str,
) -> Result<(), ProcessingError> {
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(ProcessingError {
            kind: classify_http_status(status),
            message: format!("{context} non-success status {status}: {body}"),
        });
    }
    let parsed = serde_json::from_str::<AcceptedResponse>(&body).map_err(|e| ProcessingError {
        kind: ErrorKind::Permanent,
        message: format!("parse {context} response failed: {e}"),
    })?;
    if !parsed.accepted {
        return Err(ProcessingError {
            kind: ErrorKind::Permanent,
            message: format!("{context} response not accepted"),
        });
    }
    Ok(())
}

async fn post_json_with_service_key<T: Serialize + ?Sized>(
    cfg: &Config,
    http: &reqwest::Client,
    url: &str,
    payload: &T,
    context: &str,
) -> Result<(), ProcessingError> {
    let response = http
        .post(url)
        .header("x-service-key", &cfg.service_key)
        .json(payload)
        .send()
        .await
        .map_err(|e| ProcessingError {
            kind: ErrorKind::Transient,
            message: format!("{context} request failed: {e}"),
        })?;
    handle_accepted_response(response, context).await
}

async fn post_remediation_command_projection(
    cfg: &Config,
    http: &reqwest::Client,
    payload: &RemediationCommandEnvelope,
) -> Result<RemediationProjectResponse, ProcessingError> {
    let response = http
        .post(&cfg.remediation_command_project_url)
        .header("x-service-key", &cfg.service_key)
        .json(payload)
        .send()
        .await
        .map_err(|e| ProcessingError {
            kind: ErrorKind::Transient,
            message: format!("remediation command projection request failed: {e}"),
        })?;
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(ProcessingError {
            kind: classify_http_status(status),
            message: format!("remediation command projection non-success status {status}: {body}"),
        });
    }
    let parsed =
        serde_json::from_str::<RemediationProjectResponse>(&body).map_err(|e| ProcessingError {
            kind: ErrorKind::Permanent,
            message: format!("parse remediation command projection response failed: {e}"),
        })?;
    if !parsed.accepted {
        return Err(ProcessingError {
            kind: ErrorKind::Permanent,
            message: "remediation command projection response not accepted".to_string(),
        });
    }
    Ok(parsed)
}

async fn post_json_with_rmm_server_key<T: Serialize + ?Sized>(
    cfg: &Config,
    http: &reqwest::Client,
    url: &str,
    payload: &T,
    context: &str,
) -> Result<(), ProcessingError> {
    let response = http
        .post(url)
        .header("x-rmm-server-key", &cfg.rmm_server_key)
        .json(payload)
        .send()
        .await
        .map_err(|e| ProcessingError {
            kind: ErrorKind::Transient,
            message: format!("{context} request failed: {e}"),
        })?;
    handle_accepted_response(response, context).await
}

fn classify_http_status(status: reqwest::StatusCode) -> ErrorKind {
    if status.is_server_error() || status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        ErrorKind::Transient
    } else {
        ErrorKind::Permanent
    }
}

fn build_graph_payload(
    organization_id: &str,
    agent_id: &str,
    source: &SourceMeta,
    facts: &[FactCandidate],
    event: Option<&NormalizedEvent>,
    state: &RulesResponse,
    baseline_threshold: u32,
) -> GraphBuildResult {
    let facts = if source.message_type == "snapshot" {
        complete_snapshot_absence_facts(facts, &state.current_facts, source)
    } else {
        facts.to_vec()
    };
    let facts = apply_stability_overrides(&facts, &state.stability_overrides);
    let mut current_values: HashMap<String, Value> = HashMap::new();
    for fact in &state.current_facts {
        current_values.insert(fact.fact_key.clone(), fact.fact_value.clone());
    }

    let mut baseline_by_key: HashMap<String, FactBaseline> = HashMap::new();
    for baseline in &state.baselines {
        baseline_by_key.insert(
            baseline.fact_key.clone(),
            FactBaseline {
                fact_key: baseline.fact_key.clone(),
                promoted_value: baseline.promoted_value.clone(),
                candidate_value: baseline.candidate_value.clone(),
                candidate_count: baseline.candidate_count,
                window_count: baseline.window_count,
            },
        );
    }

    let graph_facts = facts
        .iter()
        .map(|fact| GraphFact {
            fact_key: fact.fact_key.clone(),
            fact_value: fact.fact_value.clone(),
            stability_class: fact.stability_class.clone(),
            source: fact.source.clone(),
            source_ts: fact.source_ts.clone(),
        })
        .collect::<Vec<_>>();

    let mut graph_changes: Vec<GraphChange> = Vec::new();
    for fact in &facts {
        let previous = current_values
            .get(&fact.fact_key)
            .cloned()
            .unwrap_or(Value::Null);
        if !json_equal(&previous, &fact.fact_value) {
            let change_kind = if previous.is_null() {
                "insert"
            } else {
                "update"
            };
            graph_changes.push(GraphChange {
                fact_key: fact.fact_key.clone(),
                previous_value: previous,
                next_value: fact.fact_value.clone(),
                change_kind: change_kind.to_string(),
                source: fact.source.clone(),
                source_ts: fact.source_ts.clone(),
            });
        }
    }

    let mut baseline_writes: Vec<GraphBaselineWrite> = Vec::new();
    let mut baseline_shifts: Vec<BaselineShift> = Vec::new();
    for fact in facts.iter().filter(|fact| fact.stability_class == "stable") {
        let existing = baseline_by_key.get(&fact.fact_key);
        let existing_promoted = existing
            .map(|b| b.promoted_value.clone())
            .unwrap_or(Value::Null);
        let existing_candidate = existing
            .map(|b| b.candidate_value.clone())
            .unwrap_or(Value::Null);
        let mut candidate_count = existing.map(|b| b.candidate_count).unwrap_or(0);
        let mut window_count = existing.map(|b| b.window_count).unwrap_or(0);
        window_count += 1;

        let mut candidate_value = fact.fact_value.clone();
        if json_equal(&existing_candidate, &fact.fact_value) {
            candidate_count += 1;
        } else {
            candidate_value = fact.fact_value.clone();
            candidate_count = 1;
        }

        let mut promoted_value = existing_promoted.clone();
        let mut last_changed_at = None;
        let support_ratio = if window_count > 0 {
            candidate_count as f64 / window_count as f64
        } else {
            0.0
        };
        if (candidate_count as u32) >= baseline_threshold
            && !json_equal(&promoted_value, &candidate_value)
        {
            if !promoted_value.is_null() {
                baseline_shifts.push(BaselineShift {
                    fact_key: fact.fact_key.clone(),
                    current_value: candidate_value.clone(),
                    current_value_text: json_string(&candidate_value),
                    previous_value: promoted_value.clone(),
                    previous_value_text: Some(json_string(&promoted_value)),
                    support_ratio: Some(support_ratio),
                    confidence_score: Some(support_ratio),
                    scope_type: "device".to_string(),
                });
            }
            promoted_value = candidate_value.clone();
            last_changed_at = Some(source.source_ts.clone());
            candidate_count = 0;
        }

        baseline_writes.push(GraphBaselineWrite {
            fact_key: fact.fact_key.clone(),
            promoted_value,
            candidate_value,
            candidate_count,
            window_count,
            last_changed_at,
        });
    }

    // Scope drift detection: compare device baselines against scope baselines
    let mut scope_drift_candidates: Vec<BaselineShift> = Vec::new();
    for scope_bl in &state.scope_baselines {
        if !scope_bl.is_stable {
            continue;
        }
        let device_promoted = baseline_writes
            .iter()
            .find(|bw| bw.fact_key == scope_bl.fact_key)
            .map(|bw| &bw.promoted_value)
            .or_else(|| {
                baseline_by_key
                    .get(&scope_bl.fact_key)
                    .map(|b| &b.promoted_value)
            });

        if let Some(device_value) = device_promoted {
            if !device_value.is_null() && !json_equal(device_value, &scope_bl.promoted_value) {
                scope_drift_candidates.push(BaselineShift {
                    fact_key: scope_bl.fact_key.clone(),
                    current_value: device_value.clone(),
                    current_value_text: json_string(device_value),
                    previous_value: scope_bl.promoted_value.clone(),
                    previous_value_text: Some(json_string(&scope_bl.promoted_value)),
                    support_ratio: Some(scope_bl.support_ratio),
                    confidence_score: Some(scope_bl.confidence_score),
                    scope_type: scope_bl.scope_type.clone(),
                });
            }
        }
    }

    let decision = evaluate_routing_decision(
        agent_id,
        source,
        event,
        &baseline_shifts,
        &scope_drift_candidates,
        &state.rules,
        &state.recent_decisions,
    );

    let request = GraphApplyRequest {
        organization_id: organization_id.to_string(),
        agent_id: agent_id.to_string(),
        source: GraphSource {
            topic: source.topic.clone(),
            partition: source.partition,
            offset: source.offset,
            ts: source.source_ts.clone(),
            message_type: source.message_type.clone(),
        },
        facts: graph_facts,
        changes: graph_changes,
        baselines: baseline_writes,
        decision: GraphDecision {
            domain: decision.domain.clone(),
            trigger_key: decision.trigger_key.clone(),
            trigger_value: decision.trigger_value.clone(),
            action: decision.action.clone(),
            matched_rule_id: decision.matched_rule_id.clone(),
            intent_id: decision.intent_id.clone(),
            reason: decision.reason.clone(),
            dedupe_key: decision.dedupe_key.clone(),
            source: source.message_type.clone(),
            source_ts: source.source_ts.clone(),
        },
        idempotency_key: format!("{}:{}:{}", source.topic, source.partition, source.offset),
    };

    GraphBuildResult { request, decision }
}

fn complete_snapshot_absence_facts(
    facts: &[FactCandidate],
    current_facts: &[CurrentFact],
    source: &SourceMeta,
) -> Vec<FactCandidate> {
    let mut out = facts.to_vec();
    let seen_keys: HashSet<String> = out.iter().map(|fact| fact.fact_key.clone()).collect();
    for fact in current_facts {
        if seen_keys.contains(&fact.fact_key) {
            continue;
        }
        let Some(replacement_value) =
            snapshot_absence_replacement_value(&fact.fact_key, &seen_keys)
        else {
            continue;
        };
        let stability_class = fact
            .stability_class
            .clone()
            .unwrap_or_else(|| "stable".to_string());
        out.push(FactCandidate {
            fact_key: fact.fact_key.clone(),
            fact_value: replacement_value,
            stability_class,
            source: "snapshot_absence".to_string(),
            source_ts: source.source_ts.clone(),
        });
    }
    out
}

fn snapshot_absence_replacement_value(
    fact_key: &str,
    seen_keys: &HashSet<String>,
) -> Option<Value> {
    if (fact_key.starts_with("app.") && fact_key.ends_with(".installed"))
        || (fact_key.starts_with("startup.") && fact_key.ends_with(".enabled"))
        || (fact_key.starts_with("feature.") && fact_key.ends_with(".enabled"))
        || (fact_key.starts_with("task.") && fact_key.ends_with(".enabled"))
        || (fact_key.starts_with("cert.") && fact_key.ends_with(".present"))
        || (fact_key.starts_with("update.pending.") && fact_key.ends_with(".present"))
        || (fact_key.starts_with("network.adapter.") && fact_key.ends_with(".connected"))
    {
        return Some(Value::Bool(false));
    }

    if fact_key.starts_with("service.") && fact_key.ends_with(".status") {
        return Some(Value::Null);
    }

    if fact_key.starts_with("app.") && fact_key.ends_with(".version") {
        let Some(app_prefix) = fact_key.strip_suffix(".version") else {
            return None;
        };
        let installed_fact_key = format!("{app_prefix}.installed");
        if !seen_keys.contains(&installed_fact_key) {
            return Some(Value::Null);
        }
    }

    None
}

fn apply_stability_overrides(
    facts: &[FactCandidate],
    overrides: &[StabilityOverride],
) -> Vec<FactCandidate> {
    facts
        .iter()
        .cloned()
        .map(|mut fact| {
            if let Some(best_override) = overrides
                .iter()
                .filter(|override_rule| {
                    pattern_matches(&override_rule.fact_key_pattern, &fact.fact_key)
                })
                .max_by_key(|override_rule| override_specificity(&override_rule.fact_key_pattern))
            {
                fact.stability_class = best_override.stability_class.to_ascii_lowercase();
            }
            fact
        })
        .collect()
}

fn override_specificity(pattern: &str) -> usize {
    pattern
        .chars()
        .filter(|ch| *ch != '*' && *ch != '?')
        .count()
}

fn pattern_matches(pattern: &str, value: &str) -> bool {
    let pattern_chars = pattern.to_ascii_lowercase().chars().collect::<Vec<_>>();
    let value_chars = value.to_ascii_lowercase().chars().collect::<Vec<_>>();
    wildcard_match(&pattern_chars, &value_chars)
}

fn wildcard_match(pattern: &[char], value: &[char]) -> bool {
    let (mut pat_idx, mut val_idx) = (0usize, 0usize);
    let (mut star_idx, mut match_idx) = (None, 0usize);

    while val_idx < value.len() {
        if pat_idx < pattern.len()
            && (pattern[pat_idx] == '?' || pattern[pat_idx] == value[val_idx])
        {
            pat_idx += 1;
            val_idx += 1;
        } else if pat_idx < pattern.len() && pattern[pat_idx] == '*' {
            star_idx = Some(pat_idx);
            match_idx = val_idx;
            pat_idx += 1;
        } else if let Some(star_pos) = star_idx {
            pat_idx = star_pos + 1;
            match_idx += 1;
            val_idx = match_idx;
        } else {
            return false;
        }
    }

    while pat_idx < pattern.len() && pattern[pat_idx] == '*' {
        pat_idx += 1;
    }

    pat_idx == pattern.len()
}

fn evaluate_routing_decision(
    agent_id: &str,
    source: &SourceMeta,
    event: Option<&NormalizedEvent>,
    baseline_shifts: &[BaselineShift],
    scope_drift_shifts: &[BaselineShift],
    rules: &[RoutingRule],
    recent_decisions: &[RecentDecision],
) -> DecisionOutcome {
    let mut candidates: Vec<RoutingCandidate> = Vec::new();

    if let Some(event) = event {
        let primary_value = event
            .message
            .clone()
            .or_else(|| event.code.clone())
            .unwrap_or_else(|| event.severity.clone());
        candidates.push(RoutingCandidate {
            domain: "event".to_string(),
            trigger_key: format!("event:{}", event.event_type),
            current_value: Value::String(primary_value.clone()),
            current_value_text: primary_value.clone(),
            previous_value: Value::Null,
            previous_value_text: None,
            support_ratio: None,
            confidence_score: None,
            scope_type: "device".to_string(),
        });
        candidates.push(RoutingCandidate {
            domain: "event".to_string(),
            trigger_key: event.event_type.clone(),
            current_value: Value::String(primary_value.clone()),
            current_value_text: primary_value.clone(),
            previous_value: Value::Null,
            previous_value_text: None,
            support_ratio: None,
            confidence_score: None,
            scope_type: "device".to_string(),
        });
        if let Some(service_name) = &event.service_name {
            candidates.push(RoutingCandidate {
                domain: "event".to_string(),
                trigger_key: format!("event:service:{}", normalize_key_part(service_name)),
                current_value: Value::String(primary_value.clone()),
                current_value_text: primary_value.clone(),
                previous_value: Value::Null,
                previous_value_text: None,
                support_ratio: None,
                confidence_score: None,
                scope_type: "device".to_string(),
            });
        }
    }

    for shift in baseline_shifts {
        candidates.push(RoutingCandidate {
            domain: "baseline".to_string(),
            trigger_key: shift.fact_key.clone(),
            current_value: shift.current_value.clone(),
            current_value_text: shift.current_value_text.clone(),
            previous_value: shift.previous_value.clone(),
            previous_value_text: shift.previous_value_text.clone(),
            support_ratio: shift.support_ratio,
            confidence_score: shift.confidence_score,
            scope_type: shift.scope_type.clone(),
        });
    }

    for shift in scope_drift_shifts {
        candidates.push(RoutingCandidate {
            domain: "scope_drift".to_string(),
            trigger_key: shift.fact_key.clone(),
            current_value: shift.current_value.clone(),
            current_value_text: shift.current_value_text.clone(),
            previous_value: shift.previous_value.clone(),
            previous_value_text: shift.previous_value_text.clone(),
            support_ratio: shift.support_ratio,
            confidence_score: shift.confidence_score,
            scope_type: shift.scope_type.clone(),
        });
    }

    if candidates.is_empty() {
        candidates.push(RoutingCandidate {
            domain: source.message_type.clone(),
            trigger_key: "none".to_string(),
            current_value: Value::Null,
            current_value_text: "none".to_string(),
            previous_value: Value::Null,
            previous_value_text: None,
            support_ratio: None,
            confidence_score: None,
            scope_type: "device".to_string(),
        });
    }

    for rule in rules {
        let rule_domain = rule.trigger_domain.to_lowercase();
        for candidate in &candidates {
            if candidate.domain != rule_domain {
                continue;
            }
            if !pattern_matches(&rule.trigger_key, &candidate.trigger_key) {
                continue;
            }
            if !rule_match(
                &rule.match_operator,
                rule.match_value.as_deref(),
                &candidate.current_value_text,
            ) {
                continue;
            }
            if let Some(previous_operator) = rule.previous_match_operator.as_deref() {
                let previous_candidate_text =
                    candidate.previous_value_text.as_deref().unwrap_or("");
                if !rule_match(
                    previous_operator,
                    rule.previous_match_value.as_deref(),
                    previous_candidate_text,
                ) {
                    continue;
                }
            }
            if let Some(scope_filter) = rule.scope_type_filter.as_deref() {
                if !scope_filter
                    .trim()
                    .eq_ignore_ascii_case(&candidate.scope_type)
                {
                    continue;
                }
            }
            if let Some(min_support_ratio) = rule.min_support_ratio {
                if candidate.support_ratio.unwrap_or(-1.0) < min_support_ratio {
                    continue;
                }
            }
            if let Some(min_confidence_score) = rule.min_confidence_score {
                if candidate.confidence_score.unwrap_or(-1.0) < min_confidence_score {
                    continue;
                }
            }
            if !rule.cooldown_seconds.eq(&0) && is_rule_in_cooldown(rule, recent_decisions) {
                continue;
            }
            let action = normalize_action(&rule.action);
            let previous_value = if candidate.previous_value.is_null() {
                Value::Null
            } else {
                candidate.previous_value.clone()
            };
            return DecisionOutcome {
                domain: candidate.domain.clone(),
                trigger_key: candidate.trigger_key.clone(),
                trigger_value: serde_json::json!({
                    "domain": candidate.domain.clone(),
                    "triggerKey": candidate.trigger_key.clone(),
                    "currentValue": candidate.current_value.clone(),
                    "currentValueText": candidate.current_value_text.clone(),
                    "previousValue": previous_value,
                    "previousValueText": candidate.previous_value_text.clone(),
                    "supportRatio": candidate.support_ratio,
                    "confidenceScore": candidate.confidence_score,
                    "scopeType": candidate.scope_type.clone()
                }),
                action: action.clone(),
                matched_rule_id: Some(rule.id.clone()),
                intent_id: rule.intent_id.clone(),
                reason: format!("matched rule {}", rule.id),
                dedupe_key: build_dedupe_key(agent_id, &rule.id, candidate, &action),
            };
        }
    }

    let fallback = &candidates[0];
    DecisionOutcome {
        domain: fallback.domain.clone(),
        trigger_key: fallback.trigger_key.clone(),
        trigger_value: serde_json::json!({
            "domain": fallback.domain.clone(),
            "triggerKey": fallback.trigger_key.clone(),
            "currentValue": fallback.current_value.clone(),
            "currentValueText": fallback.current_value_text.clone(),
            "previousValue": fallback.previous_value.clone(),
            "previousValueText": fallback.previous_value_text.clone(),
            "supportRatio": fallback.support_ratio,
            "confidenceScore": fallback.confidence_score,
            "scopeType": fallback.scope_type.clone()
        }),
        action: "ignore".to_string(),
        matched_rule_id: None,
        intent_id: None,
        reason: "no matching rule".to_string(),
        dedupe_key: build_dedupe_key(agent_id, "none", fallback, "ignore"),
    }
}

fn rule_match(operator: &str, expected: Option<&str>, candidate: &str) -> bool {
    let op = operator.trim().to_lowercase();
    let expected = expected.unwrap_or("").trim();
    match op.as_str() {
        "exists" => !candidate.trim().is_empty(),
        "contains" => !expected.is_empty() && candidate.contains(expected),
        "not_contains" => !expected.is_empty() && !candidate.contains(expected),
        "not_equals" => !expected.is_empty() && candidate != expected,
        "starts_with" => !expected.is_empty() && candidate.starts_with(expected),
        "ends_with" => !expected.is_empty() && candidate.ends_with(expected),
        "equals" | _ => {
            if expected.is_empty() {
                true
            } else {
                candidate == expected
            }
        }
    }
}

fn normalize_action(action: &str) -> String {
    match action.trim().to_lowercase().replace('-', "_").as_str() {
        "ticket" => "ticket".to_string(),
        "recommend" => "recommend".to_string(),
        "auto_remediate" | "auto-remediate" | "autoremediate" => "auto_remediate".to_string(),
        "llm_router" => "llm_router".to_string(),
        _ => "ignore".to_string(),
    }
}

fn build_dedupe_key(
    agent_id: &str,
    rule_id: &str,
    candidate: &RoutingCandidate,
    action: &str,
) -> String {
    let previous_text = candidate
        .previous_value_text
        .clone()
        .unwrap_or_else(|| "none".to_string());
    let seed = format!(
        "{}|{}|{}|{}|{}|{}|{}",
        agent_id,
        rule_id,
        candidate.domain.as_str(),
        candidate.trigger_key.as_str(),
        candidate.current_value_text.as_str(),
        previous_text,
        action
    );
    sha256_hex(&seed)
}

fn is_rule_in_cooldown(rule: &RoutingRule, recent_decisions: &[RecentDecision]) -> bool {
    let now = Utc::now();
    recent_decisions.iter().any(|decision| {
        if decision.rule_id != rule.id {
            return false;
        }
        if let Ok(decided_at) = chrono::DateTime::parse_from_rfc3339(&decision.decided_at) {
            let elapsed = now.signed_duration_since(decided_at.with_timezone(&Utc));
            elapsed.num_seconds() < rule.cooldown_seconds as i64
        } else {
            false
        }
    })
}

fn normalize_event(
    envelope: &EventEnvelope,
    source: &SourceMeta,
) -> Result<NormalizedEvent, ProcessingError> {
    let event_obj = envelope.event.as_object().ok_or_else(|| ProcessingError {
        kind: ErrorKind::Permanent,
        message: "event field must be a JSON object".to_string(),
    })?;

    let received_at = envelope
        .received_at
        .as_deref()
        .and_then(parse_rfc3339_to_utc)
        .unwrap_or_else(Utc::now);
    let occurred_at =
        read_str_from_map(event_obj, &["occurredAt", "occurred_at", "timestamp", "ts"])
            .as_deref()
            .and_then(parse_rfc3339_to_utc)
            .unwrap_or(received_at);

    let event_type = read_str_from_map(event_obj, &["eventType", "event_type", "type", "kind"])
        .unwrap_or_else(|| "unknown".to_string());
    let severity =
        read_str_from_map(event_obj, &["severity", "level"]).unwrap_or_else(|| "info".to_string());
    let source_name =
        read_str_from_map(event_obj, &["source", "origin"]).unwrap_or_else(|| "agent".to_string());
    let service_name = read_str_from_map(event_obj, &["serviceName", "service_name", "service"]);
    let process_name = read_str_from_map(event_obj, &["processName", "process_name", "process"]);
    let code = read_str_from_map(event_obj, &["code", "errorCode", "error_code", "id"]);
    let message = read_str_from_map(event_obj, &["message", "description", "title"]);

    let event_id = read_str_from_map(event_obj, &["eventId", "event_id"]).unwrap_or_else(|| {
        let payload = json_string(&envelope.event);
        let seed = format!(
            "{}|{}|{}|{}|{}|{}",
            envelope.agent_id,
            event_type,
            occurred_at.to_rfc3339(),
            source.topic,
            source.partition,
            source.offset
        );
        sha256_hex(&format!("{seed}|{payload}"))
    });

    Ok(NormalizedEvent {
        event_id,
        occurred_at,
        received_at,
        event_type,
        severity,
        source: source_name,
        service_name,
        process_name,
        code,
        message,
        attributes: envelope.event.clone(),
    })
}

fn facts_from_snapshot(snapshot: &Value, source: &SourceMeta) -> Vec<FactCandidate> {
    let collection = snapshot.get("collection").unwrap_or(snapshot);
    let mut out: Vec<FactCandidate> = Vec::new();

    if let Some(os_name) = value_at_path(collection, &["operating_system", "system", "os", "name"])
        .or_else(|| value_at_path(collection, &["operating_system", "system", "name"]))
        .and_then(value_to_string)
    {
        out.push(FactCandidate {
            fact_key: "os.name".to_string(),
            fact_value: Value::String(os_name),
            stability_class: "stable".to_string(),
            source: "snapshot".to_string(),
            source_ts: source.source_ts.clone(),
        });
    }

    if let Some(os_version) =
        value_at_path(collection, &["operating_system", "system", "os", "version"])
            .or_else(|| value_at_path(collection, &["operating_system", "system", "version"]))
            .and_then(value_to_string)
    {
        out.push(FactCandidate {
            fact_key: "os.version".to_string(),
            fact_value: Value::String(os_version),
            stability_class: "stable".to_string(),
            source: "snapshot".to_string(),
            source_ts: source.source_ts.clone(),
        });
    }

    if let Some(os_build) =
        value_at_path(collection, &["operating_system", "system", "os", "build"])
            .and_then(value_to_string)
    {
        out.push(FactCandidate {
            fact_key: "os.build".to_string(),
            fact_value: Value::String(os_build),
            stability_class: "stable".to_string(),
            source: "snapshot".to_string(),
            source_ts: source.source_ts.clone(),
        });
    }

    if let Some(hostname) = value_at_path(collection, &["operating_system", "system", "hostname"])
        .or_else(|| value_at_path(collection, &["metadata", "hostname"]))
        .and_then(value_to_string)
    {
        out.push(FactCandidate {
            fact_key: "system.hostname".to_string(),
            fact_value: Value::String(hostname),
            stability_class: "stable".to_string(),
            source: "snapshot".to_string(),
            source_ts: source.source_ts.clone(),
        });
    }

    if let Some(domain) = value_at_path(collection, &["operating_system", "system", "domain"])
        .and_then(value_to_string)
    {
        out.push(FactCandidate {
            fact_key: "identity.domain".to_string(),
            fact_value: Value::String(domain),
            stability_class: "stable".to_string(),
            source: "snapshot".to_string(),
            source_ts: source.source_ts.clone(),
        });
    }

    if let Some(domain_joined) =
        value_at_path(collection, &["operating_system", "ad_ds", "domain_name"])
            .and_then(value_to_string)
            .map(|v| !v.is_empty())
            .or_else(|| {
                value_at_path(collection, &["operating_system", "system", "domain"])
                    .and_then(value_to_string)
                    .map(|v| !v.is_empty())
            })
    {
        out.push(FactCandidate {
            fact_key: "identity.domain_joined".to_string(),
            fact_value: Value::Bool(domain_joined),
            stability_class: "stable".to_string(),
            source: "snapshot".to_string(),
            source_ts: source.source_ts.clone(),
        });
    }

    if let Some(agent_version) =
        value_at_path(snapshot, &["metadata", "agent_version"]).and_then(value_to_string)
    {
        out.push(FactCandidate {
            fact_key: "agent.version".to_string(),
            fact_value: Value::String(agent_version),
            stability_class: "stable".to_string(),
            source: "snapshot".to_string(),
            source_ts: source.source_ts.clone(),
        });
    }

    if let Some(reboot_required) = value_at_path(
        collection,
        &[
            "operating_system",
            "updates",
            "windows_update",
            "pending_reboot",
        ],
    )
    .or_else(|| {
        value_at_path(
            collection,
            &["software", "windows_updates", "pending_reboot"],
        )
    })
    .and_then(value_to_bool)
    {
        out.push(FactCandidate {
            fact_key: "security.reboot_required".to_string(),
            fact_value: Value::Bool(reboot_required),
            stability_class: "stable".to_string(),
            source: "snapshot".to_string(),
            source_ts: source.source_ts.clone(),
        });
    }

    if let Some(pending_count) = value_at_path(
        collection,
        &[
            "operating_system",
            "updates",
            "windows_update",
            "pending_count",
        ],
    )
    .or_else(|| {
        value_at_path(
            collection,
            &["software", "windows_updates", "pending_count"],
        )
    })
    .and_then(value_to_i64)
    {
        out.push(FactCandidate {
            fact_key: "updates.pending_count".to_string(),
            fact_value: Value::Number(pending_count.into()),
            stability_class: "noisy".to_string(),
            source: "snapshot".to_string(),
            source_ts: source.source_ts.clone(),
        });
    }

    if let Some(av_enabled) = value_at_path(
        collection,
        &["security", "antivirus", "windows_defender", "enabled"],
    )
    .or_else(|| value_at_path(collection, &["security", "antivirus", "enabled"]))
    .and_then(value_to_bool)
    {
        out.push(FactCandidate {
            fact_key: "security.antivirus_enabled".to_string(),
            fact_value: Value::Bool(av_enabled),
            stability_class: "stable".to_string(),
            source: "snapshot".to_string(),
            source_ts: source.source_ts.clone(),
        });
    }

    if let Some(realtime) = value_at_path(
        collection,
        &[
            "security",
            "antivirus",
            "windows_defender",
            "real_time_protection",
        ],
    )
    .and_then(value_to_bool)
    {
        out.push(FactCandidate {
            fact_key: "security.antivirus_realtime".to_string(),
            fact_value: Value::Bool(realtime),
            stability_class: "stable".to_string(),
            source: "snapshot".to_string(),
            source_ts: source.source_ts.clone(),
        });
    }

    if let Some(uac_enabled) =
        value_at_path(collection, &["security", "uac_enabled"]).and_then(value_to_bool)
    {
        out.push(FactCandidate {
            fact_key: "security.uac_enabled".to_string(),
            fact_value: Value::Bool(uac_enabled),
            stability_class: "stable".to_string(),
            source: "snapshot".to_string(),
            source_ts: source.source_ts.clone(),
        });
    }

    if let Some(bitlocker_enabled) =
        value_at_path(collection, &["security", "bitlocker", "enabled"]).and_then(value_to_bool)
    {
        out.push(FactCandidate {
            fact_key: "security.bitlocker_enabled".to_string(),
            fact_value: Value::Bool(bitlocker_enabled),
            stability_class: "stable".to_string(),
            source: "snapshot".to_string(),
            source_ts: source.source_ts.clone(),
        });
    }

    if let Some(secure_boot) =
        value_at_path(collection, &["hardware", "secure_boot"]).and_then(value_to_bool)
    {
        out.push(FactCandidate {
            fact_key: "security.secure_boot_enabled".to_string(),
            fact_value: Value::Bool(secure_boot),
            stability_class: "stable".to_string(),
            source: "snapshot".to_string(),
            source_ts: source.source_ts.clone(),
        });
    }

    if let Some(tpm_present) =
        value_at_path(collection, &["hardware", "tpm", "present"]).and_then(value_to_bool)
    {
        out.push(FactCandidate {
            fact_key: "security.tpm_present".to_string(),
            fact_value: Value::Bool(tpm_present),
            stability_class: "stable".to_string(),
            source: "snapshot".to_string(),
            source_ts: source.source_ts.clone(),
        });
    }

    if let Some(tpm_enabled) =
        value_at_path(collection, &["hardware", "tpm", "enabled"]).and_then(value_to_bool)
    {
        out.push(FactCandidate {
            fact_key: "security.tpm_enabled".to_string(),
            fact_value: Value::Bool(tpm_enabled),
            stability_class: "stable".to_string(),
            source: "snapshot".to_string(),
            source_ts: source.source_ts.clone(),
        });
    }

    if let Some(tpm_version) =
        value_at_path(collection, &["hardware", "tpm", "version"]).and_then(value_to_string)
    {
        out.push(FactCandidate {
            fact_key: "security.tpm_version".to_string(),
            fact_value: Value::String(tpm_version),
            stability_class: "stable".to_string(),
            source: "snapshot".to_string(),
            source_ts: source.source_ts.clone(),
        });
    }

    if let Some(firewall_enabled) = firewall_enabled_from_snapshot(collection) {
        out.push(FactCandidate {
            fact_key: "security.firewall_enabled".to_string(),
            fact_value: Value::Bool(firewall_enabled),
            stability_class: "stable".to_string(),
            source: "snapshot".to_string(),
            source_ts: source.source_ts.clone(),
        });
    }

    if let Some(services) = value_at_path(collection, &["operating_system", "services", "services"])
        .and_then(|value| value.as_array())
    {
        for service in services.iter().take(256) {
            let Some(service_obj) = service.as_object() else {
                continue;
            };
            let Some(name) = read_str_from_map(service_obj, &["name"]) else {
                continue;
            };
            let Some(status) = read_str_from_map(service_obj, &["status"]) else {
                continue;
            };
            let key = format!("service.{}.status", normalize_key_part(&name));
            out.push(FactCandidate {
                fact_key: key,
                fact_value: Value::String(status.to_lowercase()),
                stability_class: "stable".to_string(),
                source: "snapshot".to_string(),
                source_ts: source.source_ts.clone(),
            });
        }
    }

    // Installed applications
    if let Some(apps) = value_at_path(collection, &["software", "installed_programs"])
        .or_else(|| value_at_path(collection, &["software", "installed_applications"]))
        .or_else(|| value_at_path(collection, &["software", "applications"]))
        .and_then(|v| v.as_array())
    {
        for app in apps.iter().take(512) {
            let Some(app_obj) = app.as_object() else {
                continue;
            };
            let Some(name) = read_str_from_map(app_obj, &["name", "app_name"]) else {
                continue;
            };
            let norm = normalize_key_part(&name);
            out.push(FactCandidate {
                fact_key: format!("app.{}.installed", norm),
                fact_value: Value::Bool(true),
                stability_class: "stable".to_string(),
                source: "snapshot".to_string(),
                source_ts: source.source_ts.clone(),
            });
            if let Some(version) = read_str_from_map(app_obj, &["version"]) {
                out.push(FactCandidate {
                    fact_key: format!("app.{}.version", norm),
                    fact_value: Value::String(version),
                    stability_class: "stable".to_string(),
                    source: "snapshot".to_string(),
                    source_ts: source.source_ts.clone(),
                });
            }
        }
    }

    // Startup items
    if let Some(items) = value_at_path(collection, &["software", "startup_items"])
        .or_else(|| value_at_path(collection, &["operating_system", "startup_items"]))
        .or_else(|| value_at_path(collection, &["operating_system", "startup", "items"]))
        .and_then(|v| v.as_array())
    {
        for item in items.iter().take(256) {
            let Some(item_obj) = item.as_object() else {
                continue;
            };
            let Some(name) = read_str_from_map(item_obj, &["name", "item_name"]) else {
                continue;
            };
            let enabled = read_str_from_map(item_obj, &["enabled", "is_enabled"])
                .and_then(|s| value_to_bool(&Value::String(s)))
                .or_else(|| {
                    item_obj
                        .get("enabled")
                        .or_else(|| item_obj.get("is_enabled"))
                        .and_then(value_to_bool)
                })
                .unwrap_or(false);
            let norm = normalize_key_part(&name);
            out.push(FactCandidate {
                fact_key: format!("startup.{}.enabled", norm),
                fact_value: Value::Bool(enabled),
                stability_class: "stable".to_string(),
                source: "snapshot".to_string(),
                source_ts: source.source_ts.clone(),
            });
        }
    }

    // Windows features
    if let Some(features) = value_at_path(collection, &["software", "features"])
        .or_else(|| value_at_path(collection, &["operating_system", "windows_features"]))
        .or_else(|| value_at_path(collection, &["operating_system", "features"]))
        .and_then(|v| v.as_array())
    {
        for feature in features.iter().take(256) {
            let Some(feature_obj) = feature.as_object() else {
                continue;
            };
            let Some(name) = read_str_from_map(feature_obj, &["name", "feature_name"]) else {
                continue;
            };
            let enabled = feature_obj
                .get("enabled")
                .and_then(value_to_bool)
                .unwrap_or(false);
            let norm = normalize_key_part(&name);
            out.push(FactCandidate {
                fact_key: format!("feature.{}.enabled", norm),
                fact_value: Value::Bool(enabled),
                stability_class: "stable".to_string(),
                source: "snapshot".to_string(),
                source_ts: source.source_ts.clone(),
            });
        }
    }

    // Scheduled tasks
    if let Some(tasks) = value_at_path(
        collection,
        &["operating_system", "scheduled_tasks", "tasks"],
    )
    .or_else(|| value_at_path(collection, &["operating_system", "scheduled_tasks"]))
    .or_else(|| value_at_path(collection, &["operating_system", "tasks"]))
    .and_then(|v| v.as_array())
    {
        for task in tasks.iter().take(256) {
            let Some(task_obj) = task.as_object() else {
                continue;
            };
            let Some(name) = read_str_from_map(task_obj, &["name", "task_name"]) else {
                continue;
            };
            let enabled = task_obj
                .get("enabled")
                .and_then(value_to_bool)
                .or_else(|| {
                    read_str_from_map(task_obj, &["state"])
                        .map(|s| matches!(s.to_lowercase().as_str(), "ready" | "running"))
                })
                .unwrap_or(false);
            let norm = normalize_key_part(&name);
            out.push(FactCandidate {
                fact_key: format!("task.{}.enabled", norm),
                fact_value: Value::Bool(enabled),
                stability_class: "stable".to_string(),
                source: "snapshot".to_string(),
                source_ts: source.source_ts.clone(),
            });
        }
    }

    // Hardware: CPU
    if let Some(cpu_model) = value_at_path(collection, &["hardware", "cpu", "model"])
        .or_else(|| value_at_path(collection, &["hardware", "cpu", "name"]))
        .and_then(value_to_string)
    {
        out.push(FactCandidate {
            fact_key: "hardware.cpu.model".to_string(),
            fact_value: Value::String(cpu_model),
            stability_class: "stable".to_string(),
            source: "snapshot".to_string(),
            source_ts: source.source_ts.clone(),
        });
    }
    if let Some(cores_physical) = value_at_path(collection, &["hardware", "cpu", "physical_cores"])
        .or_else(|| value_at_path(collection, &["hardware", "cpu", "cores"]))
        .and_then(value_to_i64)
    {
        out.push(FactCandidate {
            fact_key: "hardware.cpu.cores_physical".to_string(),
            fact_value: Value::Number(cores_physical.into()),
            stability_class: "stable".to_string(),
            source: "snapshot".to_string(),
            source_ts: source.source_ts.clone(),
        });
    }
    if let Some(cores_logical) =
        value_at_path(collection, &["hardware", "cpu", "logical_cores"]).and_then(value_to_i64)
    {
        out.push(FactCandidate {
            fact_key: "hardware.cpu.cores_logical".to_string(),
            fact_value: Value::Number(cores_logical.into()),
            stability_class: "stable".to_string(),
            source: "snapshot".to_string(),
            source_ts: source.source_ts.clone(),
        });
    }

    // Hardware: Memory
    if let Some(total_bytes) = value_at_path(collection, &["hardware", "memory", "total_bytes"])
        .or_else(|| value_at_path(collection, &["hardware", "memory", "total"]))
        .and_then(value_to_i64)
    {
        out.push(FactCandidate {
            fact_key: "hardware.memory.total_bytes".to_string(),
            fact_value: Value::Number(total_bytes.into()),
            stability_class: "stable".to_string(),
            source: "snapshot".to_string(),
            source_ts: source.source_ts.clone(),
        });
    }

    // Hardware: Disks
    let disks = value_at_path(collection, &["hardware", "disks"])
        .or_else(|| value_at_path(collection, &["hardware", "storage"]))
        .and_then(|v| v.as_array());
    if let Some(disks_arr) = disks {
        out.push(FactCandidate {
            fact_key: "hardware.disk.count".to_string(),
            fact_value: Value::Number(disks_arr.len().into()),
            stability_class: "stable".to_string(),
            source: "snapshot".to_string(),
            source_ts: source.source_ts.clone(),
        });
        for (idx, disk) in disks_arr.iter().take(32).enumerate() {
            let Some(disk_obj) = disk.as_object() else {
                continue;
            };
            let label =
                read_str_from_map(disk_obj, &["label", "name", "device_id", "drive_letter"])
                    .unwrap_or_else(|| idx.to_string());
            let norm = normalize_key_part(&label);
            if let Some(size) = disk_obj
                .get("size")
                .or_else(|| disk_obj.get("size_bytes"))
                .and_then(value_to_i64)
            {
                out.push(FactCandidate {
                    fact_key: format!("hardware.disk.{}.size_bytes", norm),
                    fact_value: Value::Number(size.into()),
                    stability_class: "stable".to_string(),
                    source: "snapshot".to_string(),
                    source_ts: source.source_ts.clone(),
                });
            }
        }
    }

    // Network: adapter count and per-adapter connected
    let adapters = value_at_path(collection, &["network", "adapters"])
        .or_else(|| value_at_path(collection, &["network", "interfaces"]))
        .and_then(|v| v.as_array());
    if let Some(adapters_arr) = adapters {
        out.push(FactCandidate {
            fact_key: "network.adapter.count".to_string(),
            fact_value: Value::Number(adapters_arr.len().into()),
            stability_class: "noisy".to_string(),
            source: "snapshot".to_string(),
            source_ts: source.source_ts.clone(),
        });
        for adapter in adapters_arr.iter().take(32) {
            let Some(adapter_obj) = adapter.as_object() else {
                continue;
            };
            let Some(name) = read_str_from_map(adapter_obj, &["name"]) else {
                continue;
            };
            let connected = adapter_obj
                .get("status")
                .and_then(value_to_string)
                .map(|s| matches!(s.to_lowercase().as_str(), "connected" | "up" | "enabled"))
                .or_else(|| adapter_obj.get("connected").and_then(value_to_bool))
                .unwrap_or(false);
            let norm = normalize_key_part(&name);
            out.push(FactCandidate {
                fact_key: format!("network.adapter.{}.connected", norm),
                fact_value: Value::Bool(connected),
                stability_class: "noisy".to_string(),
                source: "snapshot".to_string(),
                source_ts: source.source_ts.clone(),
            });
        }
    }

    // Identity: Entra joined
    if let Some(entra_joined) = value_at_path(
        collection,
        &[
            "operating_system",
            "entra_intune",
            "entra_join",
            "is_joined",
        ],
    )
    .or_else(|| {
        value_at_path(
            collection,
            &["operating_system", "entra_intune", "entra_joined"],
        )
    })
    .or_else(|| value_at_path(collection, &["operating_system", "entra", "joined"]))
    .and_then(value_to_bool)
    {
        out.push(FactCandidate {
            fact_key: "identity.entra_joined".to_string(),
            fact_value: Value::Bool(entra_joined),
            stability_class: "stable".to_string(),
            source: "snapshot".to_string(),
            source_ts: source.source_ts.clone(),
        });
    }

    // Pending updates (individual)
    if let Some(pending) = value_at_path(
        collection,
        &["operating_system", "updates", "windows_update", "pending"],
    )
    .or_else(|| {
        value_at_path(
            collection,
            &[
                "operating_system",
                "updates",
                "windows_update",
                "pending_updates",
            ],
        )
    })
    .or_else(|| {
        value_at_path(
            collection,
            &["operating_system", "updates", "pending_updates"],
        )
    })
    .and_then(|v| v.as_array())
    {
        for update in pending.iter().take(64) {
            let Some(update_obj) = update.as_object() else {
                continue;
            };
            let Some(title) = read_str_from_map(update_obj, &["title", "name", "kb"]) else {
                continue;
            };
            let norm = normalize_key_part(&title);
            out.push(FactCandidate {
                fact_key: format!("update.pending.{}.present", norm),
                fact_value: Value::Bool(true),
                stability_class: "noisy".to_string(),
                source: "snapshot".to_string(),
                source_ts: source.source_ts.clone(),
            });
        }
    }

    // Certificates
    if let Some(stores) = value_at_path(collection, &["operating_system", "certificates", "stores"])
        .and_then(|v| v.as_array())
    {
        let mut cert_count: usize = 0;
        for store in stores.iter().take(32) {
            let Some(store_obj) = store.as_object() else {
                continue;
            };
            let Some(certs) = store_obj.get("certificates").and_then(|v| v.as_array()) else {
                continue;
            };
            for cert in certs.iter().take(256) {
                let Some(cert_obj) = cert.as_object() else {
                    continue;
                };
                let Some(thumbprint) = read_str_from_map(cert_obj, &["thumbprint"]) else {
                    continue;
                };
                cert_count += 1;
                let short_thumbprint = thumbprint.chars().take(12).collect::<String>();
                let norm = normalize_key_part(&short_thumbprint);
                out.push(FactCandidate {
                    fact_key: format!("cert.{}.present", norm),
                    fact_value: Value::Bool(true),
                    stability_class: "stable".to_string(),
                    source: "snapshot".to_string(),
                    source_ts: source.source_ts.clone(),
                });
            }
        }
        out.push(FactCandidate {
            fact_key: "cert.count".to_string(),
            fact_value: Value::Number(cert_count.into()),
            stability_class: "stable".to_string(),
            source: "snapshot".to_string(),
            source_ts: source.source_ts.clone(),
        });
    } else if let Some(certs) =
        value_at_path(collection, &["operating_system", "certificates"]).and_then(|v| v.as_array())
    {
        out.push(FactCandidate {
            fact_key: "cert.count".to_string(),
            fact_value: Value::Number(certs.len().into()),
            stability_class: "stable".to_string(),
            source: "snapshot".to_string(),
            source_ts: source.source_ts.clone(),
        });
    }

    // Sessions: active count
    if let Some(sessions) = value_at_path(collection, &["operating_system", "sessions", "sessions"])
        .or_else(|| value_at_path(collection, &["operating_system", "sessions"]))
        .and_then(|v| v.as_array())
    {
        out.push(FactCandidate {
            fact_key: "session.active_count".to_string(),
            fact_value: Value::Number(sessions.len().into()),
            stability_class: "noisy".to_string(),
            source: "snapshot".to_string(),
            source_ts: source.source_ts.clone(),
        });
    }

    out
}

fn firewall_enabled_from_snapshot(collection: &Value) -> Option<bool> {
    if let Some(enabled) =
        value_at_path(collection, &["security", "firewall", "enabled"]).and_then(value_to_bool)
    {
        return Some(enabled);
    }

    let enabled_obj =
        value_at_path(collection, &["security", "firewall", "enabled"])?.as_object()?;
    let mut saw_any = false;
    let mut any_enabled = false;
    for key in ["domain", "private", "public"] {
        if let Some(value) = enabled_obj.get(key).and_then(value_to_bool) {
            saw_any = true;
            any_enabled |= value;
        }
    }
    if saw_any {
        Some(any_enabled)
    } else {
        None
    }
}

fn facts_from_event(event: &NormalizedEvent, source: &SourceMeta) -> Vec<FactCandidate> {
    let mut out = Vec::new();
    if let Some(service_name) = &event.service_name {
        let mut inferred_status = None;
        if let Some(status) =
            value_at_path(&event.attributes, &["status"]).and_then(value_to_string)
        {
            inferred_status = Some(status.to_lowercase());
        } else {
            let event_type = event.event_type.to_lowercase();
            if event_type.contains("service_stopped") || event_type.contains("service-stop") {
                inferred_status = Some("stopped".to_string());
            } else if event_type.contains("service_started") || event_type.contains("service-start")
            {
                inferred_status = Some("running".to_string());
            } else if let Some(msg) = &event.message {
                let msg_lower = msg.to_lowercase();
                if msg_lower.contains("stopped") {
                    inferred_status = Some("stopped".to_string());
                } else if msg_lower.contains("running") || msg_lower.contains("started") {
                    inferred_status = Some("running".to_string());
                }
            }
        }

        if let Some(status) = inferred_status {
            out.push(FactCandidate {
                fact_key: format!("service.{}.status", normalize_key_part(service_name)),
                fact_value: Value::String(status),
                stability_class: "stable".to_string(),
                source: "event".to_string(),
                source_ts: source.source_ts.clone(),
            });
        }
    }

    if event.event_type.to_lowercase().contains("antivirus") {
        if let Some(enabled) =
            value_at_path(&event.attributes, &["enabled"]).and_then(value_to_bool)
        {
            out.push(FactCandidate {
                fact_key: "security.antivirus_enabled".to_string(),
                fact_value: Value::Bool(enabled),
                stability_class: "stable".to_string(),
                source: "event".to_string(),
                source_ts: source.source_ts.clone(),
            });
        }
    }

    out
}

fn json_equal(a: &Value, b: &Value) -> bool {
    a == b
}

fn json_string(value: &Value) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "null".to_string())
}

fn sha256_hex(value: &str) -> String {
    let digest = Sha256::digest(value.as_bytes());
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        out.push_str(&format!("{:02x}", byte));
    }
    out
}

fn normalize_key_part(input: &str) -> String {
    input
        .chars()
        .map(|ch| match ch {
            'a'..='z' | 'A'..='Z' | '0'..='9' => ch.to_ascii_lowercase(),
            _ => '_',
        })
        .collect()
}

fn value_at_path<'a>(value: &'a Value, path: &[&str]) -> Option<&'a Value> {
    let mut current = value;
    for segment in path {
        current = current.get(*segment)?;
    }
    Some(current)
}

fn value_to_string(value: &Value) -> Option<String> {
    match value {
        Value::String(s) => {
            let trimmed = s.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            }
        }
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

fn value_to_bool(value: &Value) -> Option<bool> {
    match value {
        Value::Bool(b) => Some(*b),
        Value::String(s) => match s.trim().to_lowercase().as_str() {
            "true" | "1" | "yes" => Some(true),
            "false" | "0" | "no" => Some(false),
            _ => None,
        },
        _ => None,
    }
}

fn value_to_i64(value: &Value) -> Option<i64> {
    match value {
        Value::Number(n) => n.as_i64(),
        Value::String(s) => s.trim().parse::<i64>().ok(),
        _ => None,
    }
}

fn read_str_from_map(map: &Map<String, Value>, keys: &[&str]) -> Option<String> {
    for key in keys {
        if let Some(value) = map.get(*key) {
            if let Some(parsed) = value_to_string(value) {
                return Some(parsed);
            }
        }
    }
    None
}

fn parse_rfc3339_to_utc(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
}

fn resolve_collected_at(envelope: &SnapshotEnvelope) -> Result<DateTime<Utc>, ProcessingError> {
    if let Some(collected_at) = envelope.collected_at.as_deref() {
        return DateTime::parse_from_rfc3339(collected_at)
            .map(|value| value.with_timezone(&Utc))
            .map_err(|e| ProcessingError {
                kind: ErrorKind::Permanent,
                message: format!("invalid collectedAt: {e}"),
            });
    }

    if let Some(snapshot_ts) = envelope
        .snapshot
        .get("metadata")
        .and_then(|m| m.get("timestamp"))
        .and_then(|v| v.as_str())
    {
        return DateTime::parse_from_rfc3339(snapshot_ts)
            .map(|value| value.with_timezone(&Utc))
            .map_err(|e| ProcessingError {
                kind: ErrorKind::Permanent,
                message: format!("invalid snapshot metadata.timestamp fallback: {e}"),
            });
    }

    Err(ProcessingError {
        kind: ErrorKind::Permanent,
        message: "missing collectedAt and snapshot metadata.timestamp".into(),
    })
}

fn resolve_received_at(envelope: &SnapshotEnvelope) -> Result<DateTime<Utc>, ProcessingError> {
    if let Some(received_at) = envelope.received_at.as_deref() {
        return DateTime::parse_from_rfc3339(received_at)
            .map(|value| value.with_timezone(&Utc))
            .map_err(|e| ProcessingError {
                kind: ErrorKind::Permanent,
                message: format!("invalid receivedAt: {e}"),
            });
    }

    Ok(Utc::now())
}

async fn ensure_blob_container(
    cfg: &Config,
    http: &reqwest::Client,
) -> Result<(), ProcessingError> {
    let url = format!(
        "{}/{}?restype=container&timeout=30",
        cfg.blob_endpoint.trim_end_matches('/'),
        cfg.blob_container
    );
    let request_date = Utc::now().format("%a, %d %b %Y %H:%M:%S GMT").to_string();
    let canonical_headers = format!("x-ms-date:{}\nx-ms-version:2021-12-02", request_date);
    let canonical_resource = format!(
        "{}\nrestype:container\ntimeout:30",
        canonicalized_resource(cfg, &cfg.blob_container)
    );
    let string_to_sign = format!(
        "PUT\n\n\n\n\n\n\n\n\n\n\n\n{}\n{}",
        canonical_headers, canonical_resource
    );
    let signing_key = BASE64
        .decode(cfg.blob_account_key.as_bytes())
        .map_err(|e| ProcessingError {
            kind: ErrorKind::Permanent,
            message: format!("invalid blob account key: {e}"),
        })?;
    let mut mac = HmacSha256::new_from_slice(&signing_key).map_err(|e| ProcessingError {
        kind: ErrorKind::Permanent,
        message: format!("unable to initialize HMAC: {e}"),
    })?;
    mac.update(string_to_sign.as_bytes());
    let signature = BASE64.encode(mac.finalize().into_bytes());
    let authorization = format!("SharedKey {}:{}", cfg.blob_account_name, signature);

    let response = http
        .put(&url)
        .header("x-ms-version", "2021-12-02")
        .header("x-ms-date", request_date)
        .header("Authorization", authorization)
        .header("Content-Length", "0")
        .body(std::vec::Vec::<u8>::new())
        .send()
        .await
        .map_err(|e| ProcessingError {
            kind: ErrorKind::Transient,
            message: format!("create container request failed: {e}"),
        })?;
    match response.status().as_u16() {
        201 => {
            debug!(container = %cfg.blob_container, "blob container created");
            Ok(())
        }
        409 => {
            debug!(container = %cfg.blob_container, "blob container already exists");
            Ok(())
        }
        _ => Err(ProcessingError {
            kind: ErrorKind::Transient,
            message: format!("create container returned {}", response.status()),
        }),
    }
}

async fn upload_blob(
    cfg: &Config,
    http: &reqwest::Client,
    blob_name: &str,
    compressed: &[u8],
) -> Result<(), ProcessingError> {
    let encoded_name = blob_name
        .split('/')
        .map(|segment| encode(segment).to_string())
        .collect::<Vec<_>>()
        .join("/");
    let url = format!(
        "{}/{}/{}",
        cfg.blob_endpoint.trim_end_matches('/'),
        cfg.blob_container,
        encoded_name
    );
    let request_date = Utc::now().format("%a, %d %b %Y %H:%M:%S GMT").to_string();
    let content_length = compressed.len();
    let canonical_headers = format!(
        "x-ms-blob-type:BlockBlob\nx-ms-date:{}\nx-ms-version:2021-12-02",
        request_date
    );
    let canonical_resource =
        canonicalized_resource(cfg, &format!("{}/{}", cfg.blob_container, blob_name));
    let string_to_sign = format!(
        "PUT\ngzip\n\n{}\n\napplication/json\n\n\n\n\n\n\n{}\n{}",
        content_length, canonical_headers, canonical_resource
    );
    let signing_key = BASE64
        .decode(cfg.blob_account_key.as_bytes())
        .map_err(|e| ProcessingError {
            kind: ErrorKind::Permanent,
            message: format!("invalid blob account key: {e}"),
        })?;
    let mut mac = HmacSha256::new_from_slice(&signing_key).map_err(|e| ProcessingError {
        kind: ErrorKind::Permanent,
        message: format!("unable to initialize HMAC: {e}"),
    })?;
    mac.update(string_to_sign.as_bytes());
    let signature = BASE64.encode(mac.finalize().into_bytes());
    let authorization = format!("SharedKey {}:{}", cfg.blob_account_name, signature);

    let response = http
        .put(url)
        .header("x-ms-blob-type", "BlockBlob")
        .header("x-ms-version", "2021-12-02")
        .header("x-ms-date", request_date)
        .header("Authorization", authorization)
        .header("Content-Type", "application/json")
        .header("Content-Encoding", "gzip")
        .body(compressed.to_vec())
        .send()
        .await
        .map_err(|e| ProcessingError {
            kind: ErrorKind::Transient,
            message: format!("blob upload request failed: {e}"),
        })?;
    if response.status().is_success() {
        debug!(
            blob_name = %blob_name,
            status = %response.status(),
            bytes = compressed.len(),
            "blob upload succeeded"
        );
        return Ok(());
    }
    Err(ProcessingError {
        kind: ErrorKind::Transient,
        message: format!("blob upload returned {}", response.status()),
    })
}

fn canonicalized_resource(cfg: &Config, resource_path: &str) -> String {
    let endpoint = cfg.blob_endpoint.trim_end_matches('/');
    let path_style_emulator = endpoint.ends_with(&format!("/{}", cfg.blob_account_name));
    if path_style_emulator {
        format!(
            "/{}/{}/{}",
            cfg.blob_account_name, cfg.blob_account_name, resource_path
        )
    } else {
        format!("/{}/{}", cfg.blob_account_name, resource_path)
    }
}

fn parse_brokers(brokers: &[String]) -> Result<Vec<BrokerAddress>> {
    let mut out = Vec::with_capacity(brokers.len());
    for broker in brokers {
        let mut parts = broker.splitn(2, ':');
        let host = parts
            .next()
            .ok_or_else(|| anyhow!("invalid broker entry: {broker}"))?;
        let port_raw = parts
            .next()
            .ok_or_else(|| anyhow!("broker missing port: {broker}"))?;
        let port = port_raw
            .parse::<u16>()
            .with_context(|| format!("invalid broker port in {broker}"))?;
        out.push(BrokerAddress {
            host: host.to_string(),
            port,
        });
    }
    Ok(out)
}

fn configured_consumer_topics(cfg: &Config) -> Vec<String> {
    vec![
        cfg.snapshot_topic.clone(),
        cfg.events_topic.clone(),
        cfg.remediation_commands_topic.clone(),
        cfg.remediation_status_topic.clone(),
        cfg.patch_progress_topic.clone(),
    ]
}

async fn discover_topic_assignment(
    brokers: &[BrokerAddress],
    configured_topics: &[String],
) -> Result<TopicPartitions> {
    const METADATA_CORRELATION_ID: i32 = 1;
    const METADATA_CLIENT_ID: &str = "rmm-telemetry-partition-discovery";

    let cluster_metadata = ClusterMetadata::<TcpConnection>::new(
        brokers.to_vec(),
        METADATA_CORRELATION_ID,
        METADATA_CLIENT_ID.to_string(),
        configured_topics.to_vec(),
    )
    .await
    .map_err(|error| {
        anyhow!(
            "topic metadata discovery failed for {}: {error:?}",
            configured_topics.join(", ")
        )
    })?;

    let mut discovered_topics = HashMap::new();
    for topic in cluster_metadata.topics {
        let topic_name = String::from_utf8(topic.name.to_vec())
            .context("topic metadata contained a non-UTF-8 topic name")?;
        let partition_ids = topic
            .partitions
            .into_iter()
            .map(|partition| partition.partition_index)
            .collect();
        discovered_topics.insert(topic_name, partition_ids);
    }

    build_topic_assignment(configured_topics, &discovered_topics)
}

fn build_topic_assignment(
    configured_topics: &[String],
    discovered_topics: &HashMap<String, Vec<i32>>,
) -> Result<TopicPartitions> {
    let mut builder = TopicPartitionsBuilder::new();
    let mut invalid_topics = Vec::new();

    for topic in configured_topics {
        match discovered_topics.get(topic) {
            Some(partition_ids) if !partition_ids.is_empty() => {
                builder = builder.assign(topic.clone(), partition_ids.clone());
            }
            Some(_) => invalid_topics.push(format!("{topic} (no partitions)")),
            None => invalid_topics.push(format!("{topic} (missing)")),
        }
    }

    if !invalid_topics.is_empty() {
        return Err(anyhow!(
            "broker metadata did not provide usable partitions for configured topics: {}",
            invalid_topics.join(", ")
        ));
    }

    Ok(builder.build())
}

async fn validate_group_offsets_preflight(
    brokers: &[BrokerAddress],
    consumer_group: &str,
    assignment: &TopicPartitions,
) -> Result<()> {
    const PREFLIGHT_CORRELATION_ID: i32 = 1;
    const PREFLIGHT_CLIENT_ID: &str = "rmm-telemetry-preflight";
    const TIMESTAMP_EARLIEST: i64 = -2;
    const TIMESTAMP_LATEST: i64 = -1;

    let topics = assignment.keys().cloned().collect::<Vec<_>>();
    let cluster_metadata = ClusterMetadata::<TcpConnection>::new(
        brokers.to_vec(),
        PREFLIGHT_CORRELATION_ID,
        PREFLIGHT_CLIENT_ID.to_string(),
        topics,
    )
    .await
    .map_err(|e| anyhow!("preflight metadata fetch failed: {e:?}"))?;

    let bootstrap_conn = TcpConnection::new(brokers.to_vec())
        .await
        .map_err(|e| anyhow!("preflight bootstrap connection failed: {e:?}"))?;
    let coordinator = find_coordinator(
        bootstrap_conn,
        PREFLIGHT_CORRELATION_ID,
        PREFLIGHT_CLIENT_ID,
        consumer_group,
    )
    .await
    .map_err(|e| anyhow!("preflight coordinator lookup failed: {e:?}"))?;
    if coordinator.error_code != KafkaCode::None {
        return Err(anyhow!(
            "preflight coordinator lookup returned {:?} for group {}",
            coordinator.error_code,
            consumer_group
        ));
    }
    let coordinator_host = std::str::from_utf8(coordinator.host.as_ref())
        .context("preflight coordinator host decode failed")?;
    let coordinator_port = u16::try_from(coordinator.port)
        .with_context(|| format!("preflight invalid coordinator port {}", coordinator.port))?;
    let coordinator_conn = TcpConnection::from_addr(
        brokers.to_vec(),
        BrokerAddress {
            host: coordinator_host.to_string(),
            port: coordinator_port,
        },
    )
    .await
    .map_err(|e| anyhow!("preflight coordinator connection failed: {e:?}"))?;

    let group_offsets = fetch_offset(
        PREFLIGHT_CORRELATION_ID,
        PREFLIGHT_CLIENT_ID,
        consumer_group,
        coordinator_conn,
        assignment,
    )
    .await
    .map_err(|e| anyhow!("preflight group offset fetch failed: {e:?}"))?;
    if group_offsets.error_code != KafkaCode::None {
        return Err(anyhow!(
            "preflight group offset fetch returned {:?} for group {}",
            group_offsets.error_code,
            consumer_group
        ));
    }

    let leader_assignments = cluster_metadata
        .get_connections_for_topic_partitions(assignment)
        .map_err(|e| anyhow!("preflight leader assignment resolution failed: {e:?}"))?;

    let mut earliest_offsets: HashMap<(String, i32), i64> = HashMap::new();
    let mut latest_offsets: HashMap<(String, i32), i64> = HashMap::new();

    for (leader_conn, partitions_for_broker) in leader_assignments {
        let earliest = list_offsets(
            leader_conn.clone(),
            PREFLIGHT_CORRELATION_ID,
            PREFLIGHT_CLIENT_ID,
            &partitions_for_broker,
            TIMESTAMP_EARLIEST,
        )
        .await
        .map_err(|e| anyhow!("preflight earliest offset fetch failed: {e:?}"))?;
        for (topic_raw, partition) in earliest.into_box_iter() {
            let topic = std::str::from_utf8(topic_raw.as_ref())
                .context("preflight earliest topic decode failed")?
                .to_string();
            if partition.error_code != KafkaCode::None {
                return Err(anyhow!(
                    "preflight earliest offset fetch returned {:?} for {}[{}]",
                    partition.error_code,
                    topic,
                    partition.partition_index
                ));
            }
            earliest_offsets.insert((topic, partition.partition_index), partition.offset);
        }

        let latest = list_offsets(
            leader_conn,
            PREFLIGHT_CORRELATION_ID,
            PREFLIGHT_CLIENT_ID,
            &partitions_for_broker,
            TIMESTAMP_LATEST,
        )
        .await
        .map_err(|e| anyhow!("preflight latest offset fetch failed: {e:?}"))?;
        for (topic_raw, partition) in latest.into_box_iter() {
            let topic = std::str::from_utf8(topic_raw.as_ref())
                .context("preflight latest topic decode failed")?
                .to_string();
            if partition.error_code != KafkaCode::None {
                return Err(anyhow!(
                    "preflight latest offset fetch returned {:?} for {}[{}]",
                    partition.error_code,
                    topic,
                    partition.partition_index
                ));
            }
            latest_offsets.insert((topic, partition.partition_index), partition.offset);
        }
    }

    let mut invalid_offsets = Vec::new();
    let mut checked_partitions = 0usize;
    for (topic_raw, partition) in group_offsets.into_box_iter() {
        let topic = std::str::from_utf8(topic_raw.as_ref())
            .context("preflight group offset topic decode failed")?
            .to_string();
        if partition.error_code != KafkaCode::None {
            return Err(anyhow!(
                "preflight group offset fetch returned {:?} for {}[{}]",
                partition.error_code,
                topic,
                partition.partition_index
            ));
        }
        let key = (topic.clone(), partition.partition_index);
        let Some(log_start) = earliest_offsets.get(&key).copied() else {
            return Err(anyhow!(
                "preflight missing earliest offset for {}[{}]",
                topic,
                partition.partition_index
            ));
        };
        let Some(log_end) = latest_offsets.get(&key).copied() else {
            return Err(anyhow!(
                "preflight missing latest offset for {}[{}]",
                topic,
                partition.partition_index
            ));
        };

        checked_partitions += 1;
        let committed = partition.committed_offset;

        if committed == -1 && log_start > 0 {
            invalid_offsets.push(format!(
                "{}[{}] has no committed offset (-1) while log start is {} (log end {})",
                topic, partition.partition_index, log_start, log_end
            ));
            continue;
        }
        if committed >= 0 && committed < log_start {
            invalid_offsets.push(format!(
                "{}[{}] committed offset {} is behind log start {} (log end {})",
                topic, partition.partition_index, committed, log_start, log_end
            ));
            continue;
        }
        if committed > log_end {
            invalid_offsets.push(format!(
                "{}[{}] committed offset {} is past log end {} (log start {})",
                topic, partition.partition_index, committed, log_end, log_start
            ));
            continue;
        }
    }

    if !invalid_offsets.is_empty() {
        return Err(anyhow!(
            "consumer preflight failed for group {}: {}. Stop any running talos_telemetry_consumer and reset offsets before restarting (example: bun run telemetry:seek)",
            consumer_group,
            invalid_offsets.join("; ")
        ));
    }

    info!(
        consumer_group = %consumer_group,
        checked_partitions,
        "consumer preflight offset validation passed"
    );
    Ok(())
}

fn sanitize(input: &str) -> String {
    input
        .chars()
        .map(|ch| match ch {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' => ch,
            _ => '_',
        })
        .collect()
}

impl Config {
    fn from_env() -> Result<Self> {
        let kafka_brokers = required_env("RMM_TELEMETRY_KAFKA_BROKERS")?
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>();
        if kafka_brokers.is_empty() {
            return Err(anyhow!(
                "RMM_TELEMETRY_KAFKA_BROKERS must contain at least one broker"
            ));
        }

        let graph_apply_url = env::var("RMM_TELEMETRY_GRAPH_APPLY_URL")
            .unwrap_or_else(|_| "http://localhost:3001/rmm/telemetry/graph/apply-batch".into());
        let decision_execute_url = env::var("RMM_TELEMETRY_DECISION_EXECUTE_URL")
            .unwrap_or_else(|_| derive_decision_execute_url(&graph_apply_url));

        Ok(Self {
            kafka_brokers,
            snapshot_topic: required_env("RMM_TELEMETRY_SNAPSHOT_TOPIC")?,
            events_topic: required_env("RMM_TELEMETRY_EVENTS_TOPIC")?,
            remediation_commands_topic: env::var("RMM_TELEMETRY_REMEDIATION_COMMANDS_TOPIC")
                .unwrap_or_else(|_| "rmm_telemetry_remediation_commands".into()),
            remediation_status_topic: env::var("RMM_TELEMETRY_REMEDIATION_STATUS_TOPIC")
                .unwrap_or_else(|_| "rmm_telemetry_remediation_status".into()),
            patch_progress_topic: env::var("RMM_TELEMETRY_PATCH_PROGRESS_TOPIC")
                .unwrap_or_else(|_| "rmm_telemetry_patch_progress".into()),
            dlq_topic: env::var("RMM_TELEMETRY_SNAPSHOT_DLQ_TOPIC")
                .unwrap_or_else(|_| "rmm_telemetry_snapshots_dlq".into()),
            remediation_dlq_topic: env::var("RMM_TELEMETRY_REMEDIATION_DLQ_TOPIC")
                .unwrap_or_else(|_| "rmm_telemetry_remediation_dlq".into()),
            consumer_group: env::var("RMM_TELEMETRY_CONSUMER_GROUP")
                .unwrap_or_else(|_| "rmm-telemetry-snapshot-consumer".into()),
            consumer_session_timeout_ms: env::var("RMM_TELEMETRY_KAFKA_SESSION_TIMEOUT_MS")
                .ok()
                .and_then(|v| v.parse::<i32>().ok())
                .unwrap_or(45_000),
            consumer_rebalance_timeout_ms: env::var("RMM_TELEMETRY_KAFKA_REBALANCE_TIMEOUT_MS")
                .ok()
                .and_then(|v| v.parse::<i32>().ok())
                .unwrap_or(45_000),
            consumer_fetch_max_wait_ms: env::var("RMM_TELEMETRY_KAFKA_FETCH_MAX_WAIT_MS")
                .ok()
                .and_then(|v| v.parse::<i32>().ok())
                .unwrap_or(100),
            consumer_fetch_min_bytes: env::var("RMM_TELEMETRY_KAFKA_FETCH_MIN_BYTES")
                .ok()
                .and_then(|v| v.parse::<i32>().ok())
                .unwrap_or(1),
            consumer_fetch_max_bytes: env::var("RMM_TELEMETRY_KAFKA_FETCH_MAX_BYTES")
                .ok()
                .and_then(|v| v.parse::<i32>().ok())
                .unwrap_or(1_048_576),
            consumer_fetch_max_partition_bytes: env::var(
                "RMM_TELEMETRY_KAFKA_FETCH_MAX_PARTITION_BYTES",
            )
            .ok()
            .and_then(|v| v.parse::<i32>().ok())
            .unwrap_or(1_048_576),
            consumer_restart_backoff_ms: env::var("RMM_TELEMETRY_CONSUMER_RESTART_BACKOFF_MS")
                .ok()
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(1_000),
            manifest_url: env::var("RMM_TELEMETRY_MANIFEST_URL")
                .unwrap_or_else(|_| "http://localhost:3001/rmm/telemetry/manifest/snapshots".into()),
            events_batch_url: env::var("RMM_TELEMETRY_EVENTS_BATCH_URL")
                .unwrap_or_else(|_| "http://localhost:3001/rmm/telemetry/events/batch".into()),
            graph_apply_url,
            decision_execute_url,
            remediation_command_project_url: env::var("RMM_TELEMETRY_REMEDIATION_COMMAND_PROJECT_URL")
                .unwrap_or_else(|_| {
                    "http://localhost:3001/rmm/telemetry/remediation/commands/project".into()
                }),
            remediation_status_project_url: env::var("RMM_TELEMETRY_REMEDIATION_STATUS_PROJECT_URL")
                .unwrap_or_else(|_| {
                    "http://localhost:3001/rmm/telemetry/remediation/commands/status".into()
                }),
            patch_progress_project_url: env::var("RMM_TELEMETRY_PATCH_PROGRESS_PROJECT_URL")
                .unwrap_or_else(|_| {
                    "http://localhost:3001/rmm/telemetry/patch/progress".into()
                }),
            remediation_enqueue_url: env::var("RMM_TELEMETRY_REMEDIATION_ENQUEUE_URL")
                .unwrap_or_else(|_| {
                    "http://localhost:3002/api/rmm/internal/remediation/commands/enqueue".into()
                }),
            rules_url_base: env::var("RMM_TELEMETRY_RULES_URL_BASE")
                .unwrap_or_else(|_| "http://localhost:3001/rmm/telemetry/rules".into()),
            processed_check_url: env::var("RMM_TELEMETRY_PROCESSED_CHECK_URL")
                .unwrap_or_else(|_| "http://localhost:3001/rmm/telemetry/messages/processed".into()),
            compat_snapshot_upsert_url: env::var("RMM_TELEMETRY_COMPAT_SNAPSHOT_UPSERT_URL")
                .ok()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .or_else(|| Some("http://localhost:3001/rmm/telemetry/snapshots/upsert".into())),
            service_key: resolve_service_key()?,
            rmm_server_key: required_env("RMM_SERVER_API_KEY")?,
            max_retries: env::var("RMM_TELEMETRY_MAX_RETRIES")
                .ok()
                .and_then(|v| v.parse::<u32>().ok())
                .unwrap_or(5),
            retry_base_ms: env::var("RMM_TELEMETRY_RETRY_BASE_MS")
                .ok()
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(500),
            baseline_stability_threshold: env::var("RMM_TELEMETRY_BASELINE_STABILITY_THRESHOLD")
                .ok()
                .and_then(|v| v.parse::<u32>().ok())
                .unwrap_or(3),
            offset_commit_retention_ms: env::var("RMM_TELEMETRY_OFFSET_COMMIT_RETENTION_MS")
                .ok()
                .and_then(|v| v.parse::<i64>().ok())
                .unwrap_or(-1),
            blob_endpoint: env::var("RMM_AZURITE_BLOB_ENDPOINT")
                .unwrap_or_else(|_| "http://127.0.0.1:10000/devstoreaccount1".into()),
            blob_container: env::var("RMM_AZURITE_CONTAINER")
                .unwrap_or_else(|_| "rmm-snapshots".into()),
            blob_account_name: env::var("RMM_AZURITE_ACCOUNT_NAME")
                .unwrap_or_else(|_| "devstoreaccount1".into()),
            blob_account_key: env::var("RMM_AZURITE_ACCOUNT_KEY")
                .unwrap_or_else(|_| "Eby8vdM02xNOcqFlqUwJPLlmEtlCDXJ1OUzFT50uSRZ6IFsuFq2UVErCz4I6tq/K1SZFPTOtr/KBHBeksoGMGw==".into()),
        })
    }
}

fn derive_decision_execute_url(graph_apply_url: &str) -> String {
    if graph_apply_url.contains("/rmm/telemetry/graph/apply-batch") {
        return graph_apply_url.replace(
            "/rmm/telemetry/graph/apply-batch",
            "/rmm/telemetry/internal/decisions/execute",
        );
    }
    "http://localhost:3001/rmm/telemetry/internal/decisions/execute".to_string()
}

fn required_env(key: &str) -> Result<String> {
    let value = env::var(key).with_context(|| format!("{key} is required"))?;
    let trimmed = value.trim().to_string();
    if trimmed.is_empty() {
        return Err(anyhow!("{key} must not be empty"));
    }
    Ok(trimmed)
}

fn usable_env_value(value: Option<String>) -> Option<String> {
    value
        .map(|raw| raw.trim().to_string())
        .filter(|trimmed| !trimmed.is_empty() && trimmed != SHARED_SERVICE_KEY_PLACEHOLDER)
}

fn resolve_service_key_from_values(
    telemetry_service_key: Option<String>,
    service_key: Option<String>,
) -> Result<String> {
    usable_env_value(telemetry_service_key)
        .or_else(|| usable_env_value(service_key))
        .ok_or_else(|| {
            anyhow!(
                "RMM_TELEMETRY_SERVICE_KEY or SERVICE_KEY is required and must not be the placeholder"
            )
        })
}

fn resolve_service_key() -> Result<String> {
    resolve_service_key_from_values(
        env::var("RMM_TELEMETRY_SERVICE_KEY").ok(),
        env::var("SERVICE_KEY").ok(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn test_source() -> SourceMeta {
        SourceMeta {
            topic: "telemetry.snapshots".to_string(),
            partition: 0,
            offset: 42,
            source_ts: "2026-03-06T12:00:00Z".to_string(),
            message_type: "snapshot".to_string(),
        }
    }

    #[test]
    fn service_key_resolver_falls_back_when_telemetry_key_is_placeholder() {
        let resolved = resolve_service_key_from_values(
            Some(SHARED_SERVICE_KEY_PLACEHOLDER.to_string()),
            Some("shared-secret".to_string()),
        )
        .unwrap();

        assert_eq!(resolved, "shared-secret");
    }

    #[test]
    fn service_key_resolver_prefers_real_telemetry_key() {
        let resolved = resolve_service_key_from_values(
            Some("telemetry-secret".to_string()),
            Some("service-secret".to_string()),
        )
        .unwrap();

        assert_eq!(resolved, "telemetry-secret");
    }

    #[test]
    fn patch_progress_projection_requires_durable_identity_fields() {
        let valid = json!({
            "organizationId": "org-1",
            "agentId": "agent-1",
            "jobId": "job-1",
            "status": "running"
        });
        assert_eq!(
            parse_patch_progress(&serde_json::to_vec(&valid).unwrap()).unwrap(),
            valid
        );

        for missing in ["organizationId", "agentId", "jobId"] {
            let mut invalid = valid.clone();
            invalid.as_object_mut().unwrap().remove(missing);
            let error = parse_patch_progress(&serde_json::to_vec(&invalid).unwrap()).unwrap_err();
            assert_eq!(error.kind, ErrorKind::Permanent);
            assert!(error.message.contains("missing organizationId"));
        }
    }

    #[test]
    fn patch_progress_projection_accepts_command_id_as_the_operation_key() {
        let valid = json!({
            "organizationId": "org-1",
            "agentId": "agent-1",
            "commandId": "command-1"
        });

        assert_eq!(
            parse_patch_progress(&serde_json::to_vec(&valid).unwrap()).unwrap(),
            valid
        );
    }

    #[test]
    fn remediation_status_projection_preserves_scoped_transition_identity() {
        let status = json!({
            "commandId": "command-1",
            "organizationId": "org-1",
            "agentId": "agent-1",
            "status": "completed",
            "stepIndex": 2,
            "evidence": { "exitCode": 0 }
        });

        assert_eq!(compact_status_for_projection(status.clone()), status);
    }

    #[test]
    fn remediation_status_compaction_never_drops_transition_scope() {
        let status = json!({
            "commandId": "command-1",
            "organizationId": "org-1",
            "agentId": "agent-1",
            "status": "failed",
            "stepIndex": 1,
            "evidence": { "output": "x".repeat(33 * 1024) }
        });

        let compacted = compact_status_for_projection(status);
        assert_eq!(compacted["commandId"], "command-1");
        assert_eq!(compacted["organizationId"], "org-1");
        assert_eq!(compacted["agentId"], "agent-1");
        assert_eq!(compacted["status"], "failed");
        assert_eq!(compacted["stepIndex"], 1);
        assert_eq!(compacted["evidence"]["truncated"], true);
    }

    #[test]
    fn topic_assignment_preserves_every_discovered_partition_id() {
        let configured_topics = vec!["snapshots".to_string(), "events".to_string()];
        let discovered_topics = HashMap::from([
            ("snapshots".to_string(), vec![0, 2, 7]),
            ("events".to_string(), vec![1, 4]),
        ]);

        let assignment = build_topic_assignment(&configured_topics, &discovered_topics).unwrap();

        assert_eq!(assignment.get("snapshots"), Some(&vec![0, 2, 7]));
        assert_eq!(assignment.get("events"), Some(&vec![1, 4]));
    }

    #[test]
    fn topic_assignment_rejects_missing_configured_topics() {
        let configured_topics = vec!["snapshots".to_string(), "events".to_string()];
        let discovered_topics = HashMap::from([("snapshots".to_string(), vec![0, 1, 2])]);

        let error = build_topic_assignment(&configured_topics, &discovered_topics).unwrap_err();

        assert!(
            error.to_string().contains("events (missing)"),
            "unexpected error: {error:#}"
        );
    }

    #[test]
    fn build_graph_payload_emits_scope_drift_decision_for_stable_scope_baseline() {
        let source = test_source();
        let facts = vec![FactCandidate {
            fact_key: "app.cisco_anyconnect.installed".to_string(),
            fact_value: Value::Bool(true),
            stability_class: "stable".to_string(),
            source: "snapshot".to_string(),
            source_ts: source.source_ts.clone(),
        }];

        let state = RulesResponse {
            rules: vec![RoutingRule {
                id: "rule-1".to_string(),
                trigger_domain: "scope_drift".to_string(),
                trigger_key: "app.cisco_anyconnect.installed".to_string(),
                match_operator: "equals".to_string(),
                match_value: Some("true".to_string()),
                previous_match_operator: None,
                previous_match_value: None,
                min_support_ratio: None,
                min_confidence_score: None,
                scope_type_filter: Some("site".to_string()),
                action: "auto_remediate".to_string(),
                intent_id: Some("vpn-reinstall".to_string()),
                cooldown_seconds: 0,
            }],
            current_facts: vec![],
            baselines: vec![],
            recent_decisions: vec![],
            scope_baselines: vec![ScopeBaseline {
                fact_key: "app.cisco_anyconnect.installed".to_string(),
                promoted_value: Value::Bool(false),
                scope_type: "site".to_string(),
                support_ratio: 0.9,
                sample_size: 4,
                confidence_score: 0.95,
                is_stable: true,
            }],
            stability_overrides: vec![],
        };

        let result = build_graph_payload("org-1", "agent-1", &source, &facts, None, &state, 1);

        assert_eq!(result.decision.domain, "scope_drift");
        assert_eq!(
            result.decision.trigger_key,
            "app.cisco_anyconnect.installed"
        );
        assert_eq!(result.decision.action, "auto_remediate");
        assert_eq!(result.decision.intent_id.as_deref(), Some("vpn-reinstall"));
        assert_eq!(
            result.decision.trigger_value,
            json!({
                "domain": "scope_drift",
                "triggerKey": "app.cisco_anyconnect.installed",
                "currentValue": true,
                "currentValueText": "true",
                "previousValue": false,
                "previousValueText": "false",
                "supportRatio": 0.9,
                "confidenceScore": 0.95,
                "scopeType": "site"
            })
        );
        assert_eq!(result.request.decision.domain, "scope_drift");
    }

    #[test]
    fn evaluate_routing_decision_supports_wildcards_and_previous_value_thresholds() {
        let decision = evaluate_routing_decision(
            "agent-1",
            &test_source(),
            None,
            &[BaselineShift {
                fact_key: "app.cisco_anyconnect.installed".to_string(),
                current_value: Value::Bool(false),
                current_value_text: "false".to_string(),
                previous_value: Value::Bool(true),
                previous_value_text: Some("true".to_string()),
                support_ratio: Some(1.0),
                confidence_score: Some(1.0),
                scope_type: "device".to_string(),
            }],
            &[],
            &[RoutingRule {
                id: "rule-2".to_string(),
                trigger_domain: "baseline".to_string(),
                trigger_key: "app.*".to_string(),
                match_operator: "equals".to_string(),
                match_value: Some("false".to_string()),
                previous_match_operator: Some("equals".to_string()),
                previous_match_value: Some("true".to_string()),
                min_support_ratio: Some(0.8),
                min_confidence_score: Some(0.8),
                scope_type_filter: Some("device".to_string()),
                action: "recommend".to_string(),
                intent_id: Some("vpn-investigate".to_string()),
                cooldown_seconds: 0,
            }],
            &[],
        );

        assert_eq!(decision.action, "recommend");
        assert_eq!(decision.matched_rule_id.as_deref(), Some("rule-2"));
        assert_eq!(decision.intent_id.as_deref(), Some("vpn-investigate"));
    }

    #[test]
    fn build_graph_payload_synthesizes_false_for_missing_snapshot_presence_facts() {
        let source = test_source();
        let state = RulesResponse {
            rules: vec![],
            current_facts: vec![
                CurrentFact {
                    fact_key: "app.wiztree_v4_30.installed".to_string(),
                    fact_value: Value::Bool(true),
                    stability_class: Some("stable".to_string()),
                },
                CurrentFact {
                    fact_key: "app.wiztree_v4_30.version".to_string(),
                    fact_value: Value::String("4.30".to_string()),
                    stability_class: Some("stable".to_string()),
                },
                CurrentFact {
                    fact_key: "task.cleanup.enabled".to_string(),
                    fact_value: Value::Bool(true),
                    stability_class: Some("stable".to_string()),
                },
                CurrentFact {
                    fact_key: "service.wiztree.status".to_string(),
                    fact_value: Value::String("running".to_string()),
                    stability_class: Some("stable".to_string()),
                },
            ],
            baselines: vec![
                FactBaseline {
                    fact_key: "app.wiztree_v4_30.installed".to_string(),
                    promoted_value: Value::Bool(true),
                    candidate_value: Value::Bool(true),
                    candidate_count: 0,
                    window_count: 4,
                },
                FactBaseline {
                    fact_key: "app.wiztree_v4_30.version".to_string(),
                    promoted_value: Value::String("4.30".to_string()),
                    candidate_value: Value::String("4.30".to_string()),
                    candidate_count: 0,
                    window_count: 4,
                },
                FactBaseline {
                    fact_key: "task.cleanup.enabled".to_string(),
                    promoted_value: Value::Bool(true),
                    candidate_value: Value::Bool(true),
                    candidate_count: 0,
                    window_count: 4,
                },
                FactBaseline {
                    fact_key: "service.wiztree.status".to_string(),
                    promoted_value: Value::String("running".to_string()),
                    candidate_value: Value::String("running".to_string()),
                    candidate_count: 0,
                    window_count: 4,
                },
            ],
            recent_decisions: vec![],
            scope_baselines: vec![],
            stability_overrides: vec![],
        };

        let result = build_graph_payload("org-1", "agent-1", &source, &[], None, &state, 1);

        let emitted_fact = result
            .request
            .facts
            .iter()
            .find(|fact| fact.fact_key == "app.wiztree_v4_30.installed")
            .expect("missing synthetic installed=false fact");
        assert_eq!(emitted_fact.fact_value, Value::Bool(false));

        let baseline = result
            .request
            .baselines
            .iter()
            .find(|baseline| baseline.fact_key == "app.wiztree_v4_30.installed")
            .expect("missing baseline write for synthetic installed=false fact");
        assert_eq!(baseline.promoted_value, Value::Bool(false));

        let version_fact = result
            .request
            .facts
            .iter()
            .find(|fact| fact.fact_key == "app.wiztree_v4_30.version")
            .expect("missing synthetic app version tombstone fact");
        assert_eq!(version_fact.fact_value, Value::Null);

        let task_fact = result
            .request
            .facts
            .iter()
            .find(|fact| fact.fact_key == "task.cleanup.enabled")
            .expect("missing synthetic task enabled=false fact");
        assert_eq!(task_fact.fact_value, Value::Bool(false));

        let service_fact = result
            .request
            .facts
            .iter()
            .find(|fact| fact.fact_key == "service.wiztree.status")
            .expect("missing synthetic service status tombstone fact");
        assert_eq!(service_fact.fact_value, Value::Null);

        assert_eq!(result.decision.domain, "baseline");
        assert_eq!(result.decision.trigger_key, "app.wiztree_v4_30.installed");
    }

    #[test]
    fn membership_related_error_detects_group_loss_and_rebalance_codes() {
        assert!(is_membership_related_error(
            &KafkaConsumerError::KafkaError(KafkaCode::UnknownMemberId)
        ));
        assert!(is_membership_related_error(
            &KafkaConsumerError::KafkaError(KafkaCode::IllegalGeneration)
        ));
        assert!(is_membership_related_error(
            &KafkaConsumerError::KafkaError(KafkaCode::RebalanceInProgress)
        ));
        assert!(!is_membership_related_error(
            &KafkaConsumerError::KafkaError(KafkaCode::InvalidTopic)
        ));
    }

    #[test]
    fn batch_slow_warning_threshold_uses_two_thirds_of_session_timeout() {
        assert_eq!(
            batch_slow_warning_threshold(45_000),
            Duration::from_millis(30_000)
        );
        assert_eq!(batch_slow_warning_threshold(1), Duration::from_millis(1));
    }
}
