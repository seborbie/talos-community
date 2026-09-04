use std::env;
use std::io::Write;
use std::path::PathBuf;
use std::thread::sleep;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use chrono::{DateTime, Utc};
use flate2::write::GzEncoder;
use flate2::Compression;
use futures_util::StreamExt;
use hmac::{Hmac, Mac};
use kafka::producer::{Producer, Record, RequiredAcks};
use samsa::prelude::{
    BrokerAddress, ConsumerGroupBuilder, TcpConnection, TopicPartitionsBuilder,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::Sha256;
use tracing::{error, info, warn};
use urlencoding::encode;

#[derive(Clone)]
struct Config {
    kafka_brokers: Vec<String>,
    snapshot_topic: String,
    snapshot_dlq_topic: String,
    consumer_group: String,
    upsert_url: String,
    service_key: String,
    max_retries: u32,
    retry_base_ms: u64,
    blob_endpoint: String,
    blob_container: String,
    blob_account_name: String,
    blob_account_key: String,
}

type HmacSha256 = Hmac<Sha256>;

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct SnapshotEnvelope {
    agent_id: String,
    collected_at: String,
    received_at: String,
    snapshot: Value,
    #[serde(default)]
    snapshot_request_id: Option<String>,
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
    tracing_subscriber::fmt().with_env_filter("info").init();
    let cfg = Config::from_env()?;
    let http = reqwest::blocking::Client::new();
    let mut producer = Producer::from_hosts(cfg.kafka_brokers.clone())
        .with_required_acks(RequiredAcks::One)
        .create()
        .context("create dlq producer")?;

    let brokers = parse_broker_addresses(&cfg.kafka_brokers)?;
    // Current local Redpanda setup uses one partition for snapshots.
    let assignment = TopicPartitionsBuilder::new()
        .assign(cfg.snapshot_topic.clone(), vec![0])
        .build();
    let group_member = ConsumerGroupBuilder::<TcpConnection>::new(
        brokers,
        cfg.consumer_group.clone(),
        assignment,
    )
    .await
    .context("create consumer group builder")?
    .build()
    .await
    .context("build consumer group member")?;

    info!("rmm telemetry consumer started");
    let stream = group_member.into_stream();
    tokio::pin!(stream);

    while let Some(batch_result) = stream.next().await {
        match batch_result {
            Ok(batch) => {
                for msg in batch {
                    let payload = msg.value.to_vec();
                    if let Err(err) = process_with_retries(&cfg, &http, &payload) {
                        error!(kind=?err.kind, error=%err.message, "message failed, sending to dlq");
                        let dlq = DlqPayload {
                            topic: msg.topic_name.clone(),
                            partition: msg.partition_index,
                            offset: msg.offset as i64,
                            payload: Some(String::from_utf8_lossy(&payload).to_string()),
                            error_kind: match err.kind {
                                ErrorKind::Transient => "transient".into(),
                                ErrorKind::Permanent => "permanent".into(),
                            },
                            error_message: err.message,
                            failed_at: Utc::now().to_rfc3339(),
                        };
                        let bytes = serde_json::to_vec(&dlq).context("serialize dlq payload")?;
                        producer
                            .send(&Record::from_value(&cfg.snapshot_dlq_topic, bytes))
                            .context("publish dlq")?;
                    }
                }
            }
            Err(error) => {
                warn!(%error, "consumer stream error; retrying");
                sleep(Duration::from_millis(1000));
            }
        }
    }

    Ok(())
}

fn parse_broker_addresses(raw: &[String]) -> Result<Vec<BrokerAddress>> {
    let mut out = Vec::new();
    for broker in raw {
        let mut parts = broker.splitn(2, ':');
        let host = parts
            .next()
            .ok_or_else(|| anyhow!("invalid broker: {broker}"))?
            .trim();
        let port = parts
            .next()
            .ok_or_else(|| anyhow!("invalid broker (missing port): {broker}"))?
            .trim()
            .parse::<i32>()
            .with_context(|| format!("invalid broker port in {broker}"))?;
        out.push(BrokerAddress {
            host: host.to_string(),
            port,
        });
    }
    Ok(out)
}

fn process_with_retries(
    cfg: &Config,
    http: &reqwest::blocking::Client,
    payload: &[u8],
) -> Result<(), ProcessingError> {
    let mut attempt = 0_u32;
    loop {
        attempt += 1;
        match process_once(cfg, http, payload) {
            Ok(()) => return Ok(()),
            Err(err) if err.kind == ErrorKind::Permanent => return Err(err),
            Err(err) => {
                if attempt > cfg.max_retries {
                    return Err(err);
                }
                let backoff = cfg
                    .retry_base_ms
                    .saturating_mul(2_u64.saturating_pow(attempt - 1));
                warn!(attempt, backoff_ms=backoff, "retrying transient error");
                sleep(Duration::from_millis(backoff));
            }
        }
    }
}

fn process_once(
    cfg: &Config,
    http: &reqwest::blocking::Client,
    payload: &[u8],
) -> Result<(), ProcessingError> {
    let envelope: SnapshotEnvelope =
        serde_json::from_slice(payload).map_err(|e| ProcessingError {
            kind: ErrorKind::Permanent,
            message: format!("invalid payload: {e}"),
        })?;
    let collected =
        DateTime::parse_from_rfc3339(&envelope.collected_at).map_err(|e| ProcessingError {
            kind: ErrorKind::Permanent,
            message: format!("invalid collectedAt: {e}"),
        })?;
    let received =
        DateTime::parse_from_rfc3339(&envelope.received_at).map_err(|e| ProcessingError {
            kind: ErrorKind::Permanent,
            message: format!("invalid receivedAt: {e}"),
        })?;

    let blob_name = format!(
        "snapshots/{}/{}/{}.json.gz",
        sanitize(&envelope.agent_id),
        collected.format("%Y/%m/%d"),
        collected.format("%Y%m%dT%H%M%S%.3fZ")
    );
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    let snapshot_bytes =
        serde_json::to_vec(&envelope.snapshot).map_err(|e| ProcessingError {
            kind: ErrorKind::Permanent,
            message: format!("serialize snapshot failed: {e}"),
        })?;
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

    if cfg.blob_endpoint.trim().is_empty() {
        return Err(ProcessingError {
            kind: ErrorKind::Permanent,
            message: "blob endpoint is empty".into(),
        });
    }
    upload_blob(cfg, http, &blob_name, &compressed)?;

    let mut body = serde_json::json!({
        "agentId": envelope.agent_id,
        "collectedAt": collected.with_timezone(&Utc).to_rfc3339(),
        "receivedAt": received.with_timezone(&Utc).to_rfc3339(),
        "snapshot": envelope.snapshot,
        "blobContainer": cfg.blob_container,
        "blobName": blob_name,
        "blobContentEncoding": "gzip",
        "blobSizeBytes": compressed.len()
    });
    if let Some(ref id) = envelope.snapshot_request_id {
        body["snapshotRequestId"] = serde_json::Value::String(id.clone());
    }
    let response = http
        .post(&cfg.upsert_url)
        .header("x-service-key", &cfg.service_key)
        .json(&body)
        .send()
        .map_err(|e| ProcessingError {
            kind: ErrorKind::Transient,
            message: format!("upsert request failed: {e}"),
        })?;
    if response.status().is_server_error() {
        return Err(ProcessingError {
            kind: ErrorKind::Transient,
            message: format!("upsert 5xx status: {}", response.status()),
        });
    }
    if !response.status().is_success() {
        return Err(ProcessingError {
            kind: ErrorKind::Permanent,
            message: format!("upsert non-success status: {}", response.status()),
        });
    }
    Ok(())
}

fn upload_blob(
    cfg: &Config,
    http: &reqwest::blocking::Client,
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
    let canonical_resource = format!(
        "/{}/{}/{}",
        cfg.blob_account_name, cfg.blob_container, blob_name
    );
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
        .map_err(|e| ProcessingError {
            kind: ErrorKind::Transient,
            message: format!("blob upload request failed: {e}"),
        })?;
    if response.status().is_success() {
        return Ok(());
    }
    Err(ProcessingError {
        kind: ErrorKind::Transient,
        message: format!("blob upload returned {}", response.status()),
    })
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
        Ok(Self {
            kafka_brokers,
            snapshot_topic: required_env("RMM_TELEMETRY_SNAPSHOT_TOPIC")?,
            snapshot_dlq_topic: env::var("RMM_TELEMETRY_SNAPSHOT_DLQ_TOPIC")
                .unwrap_or_else(|_| "rmm_telemetry_snapshots_dlq".into()),
            consumer_group: env::var("RMM_TELEMETRY_CONSUMER_GROUP")
                .unwrap_or_else(|_| "rmm-telemetry-snapshot-consumer".into()),
            upsert_url: env::var("RMM_TELEMETRY_UPSERT_URL")
                .unwrap_or_else(|_| {
                    "http://localhost:3001/rmm/telemetry/snapshots/upsert".into()
                }),
            service_key: required_env("RMM_TELEMETRY_SERVICE_KEY")?,
            max_retries: env::var("RMM_TELEMETRY_MAX_RETRIES")
                .ok()
                .and_then(|v| v.parse::<u32>().ok())
                .unwrap_or(5),
            retry_base_ms: env::var("RMM_TELEMETRY_RETRY_BASE_MS")
                .ok()
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(500),
            blob_endpoint: env::var("RMM_AZURITE_BLOB_ENDPOINT")
                .unwrap_or_else(|_| "http://127.0.0.1:10000/devstoreaccount1".into()),
            blob_container: env::var("RMM_AZURITE_CONTAINER")
                .unwrap_or_else(|_| "rmm-snapshots".into()),
            blob_account_name: env::var("RMM_AZURITE_ACCOUNT_NAME")
                .unwrap_or_else(|_| "devstoreaccount1".into()),
            blob_account_key: env::var("RMM_AZURITE_ACCOUNT_KEY").unwrap_or_else(|_| "Eby8vdM02xNOcqFlqUwJPLlmEtlCDXJ1OUzFT50uSRZ6IFsuFq2UVErCz4I6tq/K1SZFPTOtr/KBHBeksoGMGw==".into()),
        })
    }
}

fn required_env(key: &str) -> Result<String> {
    let value = env::var(key).with_context(|| format!("{key} is required"))?;
    let trimmed = value.trim().to_string();
    if trimmed.is_empty() {
        return Err(anyhow!("{key} must not be empty"));
    }
    Ok(trimmed)
}
use std::env;
use std::io::Write;
use std::thread::sleep;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use chrono::{DateTime, Utc};
use flate2::write::GzEncoder;
use flate2::Compression;
use hmac::{Hmac, Mac};
use kafka::consumer::{Consumer, FetchOffset, GroupOffsetStorage};
use kafka::producer::{Producer, Record, RequiredAcks};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::Sha256;
use tracing::{error, info, warn};
use urlencoding::encode;

#[derive(Clone)]
struct Config {
    kafka_brokers: Vec<String>,
    snapshot_topic: String,
    snapshot_dlq_topic: String,
    consumer_group: String,
    upsert_url: String,
    service_key: String,
    max_retries: u32,
    retry_base_ms: u64,
    blob_endpoint: String,
    blob_container: String,
    blob_account_name: String,
    blob_account_key: String,
}

type HmacSha256 = Hmac<Sha256>;

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct SnapshotEnvelope {
    agent_id: String,
    collected_at: String,
    received_at: String,
    snapshot: Value,
    #[serde(default)]
    snapshot_request_id: Option<String>,
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

fn main() -> Result<()> {
    load_dotenv();
    tracing_subscriber::fmt().with_env_filter("info").init();
    let cfg = Config::from_env()?;
    let http = reqwest::blocking::Client::new();
    let mut consumer = Consumer::from_hosts(cfg.kafka_brokers.clone())
        .with_topic(cfg.snapshot_topic.clone())
        .with_group(cfg.consumer_group.clone())
        .with_fallback_offset(FetchOffset::Earliest)
        .with_offset_storage(Some(GroupOffsetStorage::Kafka))
        .create()
        .context("create snapshot consumer")?;
    let mut producer = Producer::from_hosts(cfg.kafka_brokers.clone())
        .with_required_acks(RequiredAcks::One)
        .create()
        .context("create dlq producer")?;

    info!("rmm telemetry consumer started");
    loop {
        let sets = match consumer.poll() {
            Ok(sets) => sets,
            Err(error) => {
                // Broker startup races / transient socket issues should not kill the consumer.
                warn!(%error, "poll kafka failed; retrying");
                sleep(Duration::from_millis(1000));
                continue;
            }
        };
        if sets.is_empty() {
            sleep(Duration::from_millis(200));
            continue;
        }

        for set in sets.iter() {
            for msg in set.messages() {
                let payload = msg.value.to_vec();
                if let Err(err) = process_with_retries(&cfg, &http, &payload) {
                    error!(kind=?err.kind, error=%err.message, "message failed, sending to dlq");
                    let dlq = DlqPayload {
                        topic: set.topic().to_string(),
                        partition: set.partition(),
                        offset: msg.offset,
                        payload: Some(String::from_utf8_lossy(&payload).to_string()),
                        error_kind: match err.kind { ErrorKind::Transient => "transient".into(), ErrorKind::Permanent => "permanent".into() },
                        error_message: err.message,
                        failed_at: Utc::now().to_rfc3339(),
                    };
                    let bytes = serde_json::to_vec(&dlq).context("serialize dlq payload")?;
                    producer
                        .send(&Record::from_value(&cfg.snapshot_dlq_topic, bytes))
                        .context("publish dlq")?;
                }
            }
            consumer.consume_messageset(set).context("consume message set")?;
        }
        consumer.commit_consumed().context("commit offsets")?;
    }
}

fn process_with_retries(cfg: &Config, http: &reqwest::blocking::Client, payload: &[u8]) -> Result<(), ProcessingError> {
    let mut attempt = 0_u32;
    loop {
        attempt += 1;
        match process_once(cfg, http, payload) {
            Ok(()) => return Ok(()),
            Err(err) if err.kind == ErrorKind::Permanent => return Err(err),
            Err(err) => {
                if attempt > cfg.max_retries {
                    return Err(err);
                }
                let backoff = cfg.retry_base_ms.saturating_mul(2_u64.saturating_pow(attempt - 1));
                warn!(attempt, backoff_ms=backoff, "retrying transient error");
                sleep(Duration::from_millis(backoff));
            }
        }
    }
}

fn process_once(cfg: &Config, http: &reqwest::blocking::Client, payload: &[u8]) -> Result<(), ProcessingError> {
    let envelope: SnapshotEnvelope = serde_json::from_slice(payload).map_err(|e| ProcessingError { kind: ErrorKind::Permanent, message: format!("invalid payload: {e}") })?;
    let collected = DateTime::parse_from_rfc3339(&envelope.collected_at).map_err(|e| ProcessingError { kind: ErrorKind::Permanent, message: format!("invalid collectedAt: {e}") })?;
    let received = DateTime::parse_from_rfc3339(&envelope.received_at).map_err(|e| ProcessingError { kind: ErrorKind::Permanent, message: format!("invalid receivedAt: {e}") })?;

    let blob_name = format!("snapshots/{}/{}/{}.json.gz", sanitize(&envelope.agent_id), collected.format("%Y/%m/%d"), collected.format("%Y%m%dT%H%M%S%.3fZ"));
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    let snapshot_bytes = serde_json::to_vec(&envelope.snapshot).map_err(|e| ProcessingError { kind: ErrorKind::Permanent, message: format!("serialize snapshot failed: {e}") })?;
    encoder.write_all(&snapshot_bytes).map_err(|e| ProcessingError { kind: ErrorKind::Permanent, message: format!("gzip encode failed: {e}") })?;
    let compressed = encoder.finish().map_err(|e| ProcessingError { kind: ErrorKind::Permanent, message: format!("gzip finish failed: {e}") })?;

    if cfg.blob_endpoint.trim().is_empty() {
        return Err(ProcessingError { kind: ErrorKind::Permanent, message: "blob endpoint is empty".into() });
    }
    upload_blob(cfg, http, &blob_name, &compressed)?;

    let mut body = serde_json::json!({
        "agentId": envelope.agent_id,
        "collectedAt": collected.with_timezone(&Utc).to_rfc3339(),
        "receivedAt": received.with_timezone(&Utc).to_rfc3339(),
        "snapshot": envelope.snapshot,
        "blobContainer": cfg.blob_container,
        "blobName": blob_name,
        "blobContentEncoding": "gzip",
        "blobSizeBytes": compressed.len()
    });
    if let Some(ref id) = envelope.snapshot_request_id {
        body["snapshotRequestId"] = serde_json::Value::String(id.clone());
    }
    let response = http.post(&cfg.upsert_url).header("x-service-key", &cfg.service_key).json(&body).send().map_err(|e| ProcessingError { kind: ErrorKind::Transient, message: format!("upsert request failed: {e}") })?;
    if response.status().is_server_error() {
        return Err(ProcessingError { kind: ErrorKind::Transient, message: format!("upsert 5xx status: {}", response.status()) });
    }
    if !response.status().is_success() {
        return Err(ProcessingError { kind: ErrorKind::Permanent, message: format!("upsert non-success status: {}", response.status()) });
    }
    Ok(())
}

fn upload_blob(
    cfg: &Config,
    http: &reqwest::blocking::Client,
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
    let canonical_resource = format!(
        "/{}/{}/{}",
        cfg.blob_account_name, cfg.blob_container, blob_name
    );
    let string_to_sign = format!(
        "PUT\ngzip\n\n{}\n\napplication/json\n\n\n\n\n\n\n{}\n{}",
        content_length, canonical_headers, canonical_resource
    );
    let signing_key = BASE64.decode(cfg.blob_account_key.as_bytes()).map_err(|e| ProcessingError {
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
        .map_err(|e| ProcessingError {
            kind: ErrorKind::Transient,
            message: format!("blob upload request failed: {e}"),
        })?;
    if response.status().is_success() {
        return Ok(());
    }
    Err(ProcessingError {
        kind: ErrorKind::Transient,
        message: format!("blob upload returned {}", response.status()),
    })
}

fn sanitize(input: &str) -> String {
    input.chars().map(|ch| match ch { 'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' => ch, _ => '_' }).collect()
}

impl Config {
    fn from_env() -> Result<Self> {
        let kafka_brokers = required_env("RMM_TELEMETRY_KAFKA_BROKERS")?.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect::<Vec<_>>();
        if kafka_brokers.is_empty() {
            return Err(anyhow!("RMM_TELEMETRY_KAFKA_BROKERS must contain at least one broker"));
        }
        Ok(Self {
            kafka_brokers,
            snapshot_topic: required_env("RMM_TELEMETRY_SNAPSHOT_TOPIC")?,
            snapshot_dlq_topic: env::var("RMM_TELEMETRY_SNAPSHOT_DLQ_TOPIC").unwrap_or_else(|_| "rmm_telemetry_snapshots_dlq".into()),
            consumer_group: env::var("RMM_TELEMETRY_CONSUMER_GROUP").unwrap_or_else(|_| "rmm-telemetry-snapshot-consumer".into()),
            upsert_url: env::var("RMM_TELEMETRY_UPSERT_URL").unwrap_or_else(|_| "http://localhost:3001/rmm/telemetry/snapshots/upsert".into()),
            service_key: required_env("RMM_TELEMETRY_SERVICE_KEY")?,
            max_retries: env::var("RMM_TELEMETRY_MAX_RETRIES").ok().and_then(|v| v.parse::<u32>().ok()).unwrap_or(5),
            retry_base_ms: env::var("RMM_TELEMETRY_RETRY_BASE_MS").ok().and_then(|v| v.parse::<u64>().ok()).unwrap_or(500),
            blob_endpoint: env::var("RMM_AZURITE_BLOB_ENDPOINT").unwrap_or_else(|_| "http://127.0.0.1:10000/devstoreaccount1".into()),
            blob_container: env::var("RMM_AZURITE_CONTAINER").unwrap_or_else(|_| "rmm-snapshots".into()),
            blob_account_name: env::var("RMM_AZURITE_ACCOUNT_NAME").unwrap_or_else(|_| "devstoreaccount1".into()),
            blob_account_key: env::var("RMM_AZURITE_ACCOUNT_KEY").unwrap_or_else(|_| "Eby8vdM02xNOcqFlqUwJPLlmEtlCDXJ1OUzFT50uSRZ6IFsuFq2UVErCz4I6tq/K1SZFPTOtr/KBHBeksoGMGw==".into()),
        })
    }
}

fn required_env(key: &str) -> Result<String> {
    let value = env::var(key).with_context(|| format!("{key} is required"))?;
    let trimmed = value.trim().to_string();
    if trimmed.is_empty() {
        return Err(anyhow!("{key} must not be empty"));
    }
    Ok(trimmed)
}
