//! RMM Collector - Comprehensive Windows endpoint data collection
//!
//! This crate provides a modular system for collecting detailed information
//! from Windows endpoints including hardware, software, network, security,
//! and cloud management (Entra/Intune) data.

pub mod collectors;
pub mod error;
pub mod event_stream;
pub mod models;
pub mod snapshot_v2;
pub mod windows_utils;

pub use snapshot_v2 as snapshot;

pub use collectors::{CollectionConfig, CollectionOrchestrator, Collector};
pub use error::CollectorError;
pub use models::*;

use serde::{Deserialize, Serialize};

/// Metadata about a collection run
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollectionMetadata {
    pub collection_id: String,
    pub collection_time: chrono::DateTime<chrono::Utc>,
    pub agent_version: String,
    pub collection_profile: String,
    pub hostname: String,
    pub agent_id: String,
    pub collection_duration_ms: u64,
    pub errors: Vec<CollectorError>,
}

impl CollectionMetadata {
    pub fn new(agent_id: String, agent_version: String) -> Self {
        Self {
            collection_id: uuid::Uuid::new_v4().to_string(),
            collection_time: chrono::Utc::now(),
            agent_version,
            collection_profile: "full".to_string(),
            hostname: hostname::get()
                .map(|h| h.to_string_lossy().to_string())
                .unwrap_or_else(|_| "unknown".to_string()),
            agent_id,
            collection_duration_ms: 0,
            errors: Vec::new(),
        }
    }
}

/// Operating system–related collection data (system identity, services, updates, events, etc.)
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OperatingSystemInfo {
    pub system: Option<SystemInfo>,
    pub services: Option<ServicesInfo>,
    pub updates: Option<UpdatesInfo>,
    pub entra_intune: Option<EntraIntuneInfo>,
    pub events: Option<EventsSummary>,
    pub certificates: Option<CertificatesInfo>,
    pub scheduled_tasks: Option<ScheduledTasksInfo>,
    pub sessions: Option<SessionsInfo>,
    pub printers: Option<PrintersInfo>,
    pub iis: Option<IisInfo>,
    pub dhcp_server: Option<DhcpServerInfo>,
    pub dns_server: Option<DnsServerInfo>,
    pub ad_ds: Option<AdDsInfo>,
}

/// Complete collection result containing all data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FullCollection {
    pub metadata: CollectionMetadata,
    pub operating_system: Option<OperatingSystemInfo>,
    pub hardware: Option<HardwareInfo>,
    pub network: Option<NetworkInfo>,
    pub software: Option<SoftwareInfo>,
    pub security: Option<SecurityInfo>,
}

impl FullCollection {
    pub fn new(metadata: CollectionMetadata) -> Self {
        Self {
            metadata,
            operating_system: None,
            hardware: None,
            network: None,
            software: None,
            security: None,
        }
    }
}

/// Initialize tracing and common setup
pub fn init() {
    tracing_subscriber::fmt::init();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_collection_metadata() {
        let meta = CollectionMetadata::new("test-agent".to_string(), "0.1.0".to_string());
        assert!(!meta.collection_id.is_empty());
        assert_eq!(meta.agent_version, "0.1.0");
        assert_eq!(meta.agent_id, "test-agent");
    }
}
