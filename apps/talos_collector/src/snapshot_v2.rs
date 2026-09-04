use crate::{CollectionOrchestrator, FullCollection};
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotMetadata {
    pub agent_id: String,
    pub device_name: String,
    pub boot_session_id: String,
    pub agent_version: String,
    pub collection_profile: String,
    pub timestamp: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotDocument {
    pub metadata: SnapshotMetadata,
    pub collection: FullCollection,
}

#[derive(Debug, Clone)]
pub struct SnapshotConfig {
    pub agent_id: String,
    pub device_name: String,
    pub boot_session_id: String,
    pub output_path: PathBuf,
}

impl SnapshotConfig {
    pub fn default_output_path() -> PathBuf {
        PathBuf::from(r"C:\temp\rmm_full_snapshot.json")
    }
}

pub async fn run_full_snapshot(config: SnapshotConfig) -> Result<PathBuf> {
    let orchestrator = CollectionOrchestrator::full_collection();
    let agent_version = env!("CARGO_PKG_VERSION").to_string();
    let collection = orchestrator
        .collect_all(config.agent_id.clone(), agent_version.clone())
        .await
        .map_err(|e| anyhow::anyhow!("collection failed: {}", e))?;

    let snapshot = SnapshotDocument {
        metadata: SnapshotMetadata {
            agent_id: config.agent_id,
            device_name: config.device_name,
            boot_session_id: config.boot_session_id,
            agent_version,
            collection_profile: "full".to_string(),
            timestamp: Utc::now(),
        },
        collection,
    };

    let output_path = config.output_path;
    ensure_parent_dir(&output_path).await?;
    let json_output =
        serde_json::to_string_pretty(&snapshot).context("failed to serialize snapshot to json")?;

    // TODO(gzip): In production, compress json_output and write to rmm_full_snapshot.json.gz.
    tokio::fs::write(&output_path, json_output)
        .await
        .with_context(|| format!("failed to write snapshot to {}", output_path.display()))?;

    Ok(output_path)
}

async fn ensure_parent_dir(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            tokio::fs::create_dir_all(parent)
                .await
                .with_context(|| format!("failed to create directory {}", parent.display()))?;
        }
    }
    Ok(())
}
