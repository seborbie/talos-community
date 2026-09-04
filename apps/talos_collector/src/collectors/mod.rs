use crate::error::CollectorError;
use crate::models::*;
use crate::{CollectionMetadata, FullCollection, OperatingSystemInfo};
use async_trait::async_trait;
use futures::future::join_all;
use serde_json::Value;
use std::time::{Duration, Instant};
use tracing::{debug, error, info, warn};

// Collector modules
pub mod ad_ds;
pub mod certificates;
pub mod dhcp_server;
pub mod dns_server;
pub mod entra_intune;
pub mod events;
pub mod hardware;
pub mod iis;
pub mod network;
pub mod office365;
pub mod onedrive;
pub mod print_server;
pub mod scheduled_tasks;
pub mod security;
pub mod services;
pub mod sessions;
pub mod software;
pub mod system;
pub mod updates;

pub use ad_ds::AdDsCollector;
pub use certificates::CertificatesCollector;
pub use dhcp_server::DhcpServerCollector;
pub use dns_server::DnsServerCollector;
pub use entra_intune::EntraIntuneCollector;
pub use events::EventsCollector;
pub use hardware::HardwareCollector;
pub use iis::IisCollector;
pub use network::NetworkCollector;
pub use office365::Office365Collector;
pub use onedrive::OneDriveCollector;
pub use print_server::PrintServerCollector;
pub use scheduled_tasks::ScheduledTasksCollector;
pub use security::SecurityCollector;
pub use services::ServicesCollector;
pub use sessions::SessionsCollector;
pub use software::SoftwareCollector;
pub use system::SystemCollector;
pub use updates::UpdatesCollector;

/// Configuration for a collection run
#[derive(Debug, Clone)]
pub struct CollectionConfig {
    /// Timeout for each individual collector
    pub collector_timeout_secs: u64,
    /// Whether to collect in parallel or sequential
    pub parallel: bool,
    /// List of collectors to run (empty = all)
    pub include_collectors: Vec<String>,
    /// List of collectors to skip
    pub exclude_collectors: Vec<String>,
    /// Whether to continue on error
    pub continue_on_error: bool,
}

impl Default for CollectionConfig {
    fn default() -> Self {
        Self {
            collector_timeout_secs: 120,
            parallel: true,
            include_collectors: Vec::new(),
            exclude_collectors: Vec::new(),
            continue_on_error: true,
        }
    }
}

/// Trait that all collectors must implement
#[async_trait]
pub trait Collector: Send + Sync {
    /// Returns the unique name of this collector
    fn name(&self) -> &'static str;

    /// Collect data and return as JSON Value
    async fn collect(&self) -> anyhow::Result<Value>;

    /// Returns the type of data this collector produces
    fn data_type(&self) -> &'static str;

    /// Estimate collection time in milliseconds
    fn estimated_duration_ms(&self) -> u64 {
        1000
    }

    /// Whether this collector requires admin privileges
    fn requires_admin(&self) -> bool {
        false
    }

    /// Whether this collector is supported on the current platform
    fn is_supported(&self) -> bool {
        cfg!(target_os = "windows")
    }
}

/// Result from a single collector
#[derive(Debug, Clone)]
pub struct CollectorResult {
    pub name: String,
    pub success: bool,
    pub data: Option<Value>,
    pub error: Option<CollectorError>,
    pub duration_ms: u64,
}

/// Orchestrates multiple collectors
pub struct CollectionOrchestrator {
    collectors: Vec<Box<dyn Collector>>,
    config: CollectionConfig,
}

impl CollectionOrchestrator {
    /// Create a new orchestrator with default configuration
    pub fn new() -> Self {
        Self {
            collectors: Vec::new(),
            config: CollectionConfig::default(),
        }
    }

    /// Create an orchestrator with the full set of collectors
    pub fn full_collection() -> Self {
        let mut orchestrator = Self::new();
        orchestrator.add_full_collection();
        orchestrator
    }

    /// Add the full set of collectors
    pub fn add_full_collection(&mut self) {
        self.add_collector(Box::new(SystemCollector));
        self.add_collector(Box::new(HardwareCollector));
        self.add_collector(Box::new(NetworkCollector));
        self.add_collector(Box::new(CertificatesCollector));
        self.add_collector(Box::new(ScheduledTasksCollector));
        self.add_collector(Box::new(SessionsCollector));
        self.add_collector(Box::new(PrintServerCollector));
        self.add_collector(Box::new(IisCollector));
        self.add_collector(Box::new(DhcpServerCollector));
        self.add_collector(Box::new(DnsServerCollector));
        self.add_collector(Box::new(AdDsCollector));
        self.add_collector(Box::new(SoftwareCollector));
        self.add_collector(Box::new(Office365Collector));
        self.add_collector(Box::new(SecurityCollector));
        self.add_collector(Box::new(ServicesCollector));
        self.add_collector(Box::new(UpdatesCollector));
        self.add_collector(Box::new(EntraIntuneCollector));
        self.add_collector(Box::new(OneDriveCollector));
        self.add_collector(Box::new(EventsCollector));
    }

    /// Add a collector to the orchestrator
    pub fn add_collector(&mut self, collector: Box<dyn Collector>) {
        self.collectors.push(collector);
    }

    /// Set configuration
    pub fn with_config(mut self, config: CollectionConfig) -> Self {
        self.config = config;
        self
    }

    /// Run all collectors and aggregate results
    pub async fn collect_all(
        &self,
        agent_id: String,
        agent_version: String,
    ) -> Result<FullCollection, CollectorError> {
        let start_time = Instant::now();
        let metadata = CollectionMetadata::new(agent_id, agent_version);
        let mut collection = FullCollection::new(metadata);
        let mut errors: Vec<CollectorError> = Vec::new();

        // Filter collectors based on config
        let collectors_to_run: Vec<&Box<dyn Collector>> = self
            .collectors
            .iter()
            .filter(|c| {
                // Check if included (if include list is empty, include all)
                let included = self.config.include_collectors.is_empty()
                    || self
                        .config
                        .include_collectors
                        .contains(&c.name().to_string());
                // Check if not excluded
                let not_excluded = !self
                    .config
                    .exclude_collectors
                    .contains(&c.name().to_string());
                // Check platform support
                let supported = c.is_supported();

                if !supported {
                    debug!(
                        collector = c.name(),
                        "Collector not supported on this platform"
                    );
                }

                included && not_excluded && supported
            })
            .collect();

        info!(
            total_collectors = self.collectors.len(),
            running = collectors_to_run.len(),
            "Starting collection run"
        );

        let collectors_scheduled = collectors_to_run.len();
        let mut collectors_ok = 0usize;

        if self.config.parallel {
            // Run collectors in parallel with timeout
            let timeout = Duration::from_secs(self.config.collector_timeout_secs);
            let futures: Vec<_> = collectors_to_run
                .iter()
                .map(|c| self.run_collector_with_timeout(c, timeout))
                .collect();

            let results = join_all(futures).await;

            for result in results {
                match result {
                    Ok(collector_result) => {
                        if collector_result.success {
                            collectors_ok += 1;
                            self.apply_result(&mut collection, &collector_result);
                        } else if let Some(err) = collector_result.error {
                            warn!(
                                collector = collector_result.name,
                                error = %err,
                                "Collector failed"
                            );
                            if self.config.continue_on_error {
                                errors.push(err);
                            } else {
                                return Err(err);
                            }
                        }
                    }
                    Err(e) => {
                        error!(error = %e, "Unexpected error running collector");
                        if !self.config.continue_on_error {
                            return Err(CollectorError::other(
                                "orchestrator",
                                format!("Unexpected error: {}", e),
                            ));
                        }
                    }
                }
            }
        } else {
            // Run collectors sequentially
            for collector in collectors_to_run {
                let timeout = Duration::from_secs(self.config.collector_timeout_secs);
                match self.run_collector_with_timeout(collector, timeout).await {
                    Ok(result) => {
                        if result.success {
                            collectors_ok += 1;
                            self.apply_result(&mut collection, &result);
                        } else if let Some(err) = result.error {
                            warn!(collector = result.name, error = %err, "Collector failed");
                            if self.config.continue_on_error {
                                errors.push(err);
                            } else {
                                return Err(err);
                            }
                        }
                    }
                    Err(e) => {
                        error!(collector = collector.name(), error = %e, "Collector error");
                        if !self.config.continue_on_error {
                            return Err(CollectorError::other(
                                collector.name(),
                                format!("Unexpected error: {}", e),
                            ));
                        }
                    }
                }
            }
        }

        let duration = start_time.elapsed();
        collection.metadata.collection_duration_ms = duration.as_millis() as u64;
        collection.metadata.errors = errors;

        info!(
            duration_ms = collection.metadata.collection_duration_ms,
            errors = collection.metadata.errors.len(),
            collectors_ok,
            collectors_scheduled,
            "Collection run completed"
        );

        Ok(collection)
    }

    /// Run a single collector with timeout
    async fn run_collector_with_timeout(
        &self,
        collector: &Box<dyn Collector>,
        timeout: Duration,
    ) -> anyhow::Result<CollectorResult> {
        let name = collector.name();
        let start = Instant::now();

        debug!(collector = name, "Starting collector");

        let result = tokio::time::timeout(timeout, collector.collect()).await;

        let duration_ms = start.elapsed().as_millis() as u64;

        match result {
            Ok(Ok(data)) => {
                debug!(collector = name, duration_ms, "Collector succeeded");
                Ok(CollectorResult {
                    name: name.to_string(),
                    success: true,
                    data: Some(data),
                    error: None,
                    duration_ms,
                })
            }
            Ok(Err(e)) => {
                let error = CollectorError::other(name, e.to_string());
                warn!(collector = name, error = %error, "Collector returned error");
                Ok(CollectorResult {
                    name: name.to_string(),
                    success: false,
                    data: None,
                    error: Some(error),
                    duration_ms,
                })
            }
            Err(_) => {
                let error = CollectorError::timeout(name, timeout.as_millis() as u64);
                warn!(
                    collector = name,
                    timeout_ms = timeout.as_millis(),
                    "Collector timed out"
                );
                Ok(CollectorResult {
                    name: name.to_string(),
                    success: false,
                    data: None,
                    error: Some(error),
                    duration_ms: timeout.as_millis() as u64,
                })
            }
        }
    }

    /// Apply a collector result to the full collection
    fn apply_result(&self, collection: &mut FullCollection, result: &CollectorResult) {
        if let Some(ref data) = result.data {
            // Deserialize into typed field
            match result.name.as_str() {
                "System" => {
                    if let Ok(info) = serde_json::from_value::<SystemInfo>(data.clone()) {
                        collection
                            .operating_system
                            .get_or_insert_with(OperatingSystemInfo::default)
                            .system = Some(info);
                    }
                }
                "Hardware" => {
                    if let Ok(info) = serde_json::from_value::<HardwareInfo>(data.clone()) {
                        collection.hardware = Some(info);
                    }
                }
                "Network" => {
                    if let Ok(info) = serde_json::from_value::<NetworkInfo>(data.clone()) {
                        collection.network = Some(info);
                    }
                }
                "Software" => {
                    if let Ok(mut info) = serde_json::from_value::<SoftwareInfo>(data.clone()) {
                        if let Some(existing) = &collection.software {
                            info.office_365 = existing.office_365.clone();
                            info.onedrive = existing.onedrive.clone();
                        }
                        collection.software = Some(info);
                    }
                }
                "Office365" => {
                    if let Ok(info) = serde_json::from_value::<Office365Info>(data.clone()) {
                        collection
                            .software
                            .get_or_insert_with(SoftwareInfo::default)
                            .office_365 = Some(info);
                    }
                }
                "Security" => {
                    if let Ok(info) = serde_json::from_value::<SecurityInfo>(data.clone()) {
                        collection.security = Some(info);
                    }
                }
                "Services" => {
                    if let Ok(info) = serde_json::from_value::<ServicesInfo>(data.clone()) {
                        collection
                            .operating_system
                            .get_or_insert_with(OperatingSystemInfo::default)
                            .services = Some(info);
                    }
                }
                "Updates" => {
                    if let Ok(info) = serde_json::from_value::<UpdatesInfo>(data.clone()) {
                        collection
                            .operating_system
                            .get_or_insert_with(OperatingSystemInfo::default)
                            .updates = Some(info);
                    }
                }
                "EntraIntune" => {
                    if let Ok(info) = serde_json::from_value::<EntraIntuneInfo>(data.clone()) {
                        collection
                            .operating_system
                            .get_or_insert_with(OperatingSystemInfo::default)
                            .entra_intune = Some(info);
                    }
                }
                "OneDrive" => {
                    if let Ok(info) = serde_json::from_value::<OneDriveInfo>(data.clone()) {
                        collection
                            .software
                            .get_or_insert_with(SoftwareInfo::default)
                            .onedrive = Some(info);
                    }
                }
                "Events" => {
                    if let Ok(info) = serde_json::from_value::<EventsSummary>(data.clone()) {
                        collection
                            .operating_system
                            .get_or_insert_with(OperatingSystemInfo::default)
                            .events = Some(info);
                    }
                }
                "Certificates" => {
                    if let Ok(info) = serde_json::from_value::<CertificatesInfo>(data.clone()) {
                        collection
                            .operating_system
                            .get_or_insert_with(OperatingSystemInfo::default)
                            .certificates = Some(info);
                    }
                }
                "ScheduledTasks" => {
                    if let Ok(info) = serde_json::from_value::<ScheduledTasksInfo>(data.clone()) {
                        collection
                            .operating_system
                            .get_or_insert_with(OperatingSystemInfo::default)
                            .scheduled_tasks = Some(info);
                    }
                }
                "Sessions" => {
                    if let Ok(info) = serde_json::from_value::<SessionsInfo>(data.clone()) {
                        collection
                            .operating_system
                            .get_or_insert_with(OperatingSystemInfo::default)
                            .sessions = Some(info);
                    }
                }
                "Printers" => {
                    if let Ok(info) = serde_json::from_value::<PrintersInfo>(data.clone()) {
                        collection
                            .operating_system
                            .get_or_insert_with(OperatingSystemInfo::default)
                            .printers = Some(info);
                    }
                }
                "IIS" => {
                    if let Ok(info) = serde_json::from_value::<IisInfo>(data.clone()) {
                        collection
                            .operating_system
                            .get_or_insert_with(OperatingSystemInfo::default)
                            .iis = Some(info);
                    }
                }
                "DhcpServer" => {
                    if let Ok(info) = serde_json::from_value::<DhcpServerInfo>(data.clone()) {
                        collection
                            .operating_system
                            .get_or_insert_with(OperatingSystemInfo::default)
                            .dhcp_server = Some(info);
                    }
                }
                "DnsServer" => {
                    if let Ok(info) = serde_json::from_value::<DnsServerInfo>(data.clone()) {
                        collection
                            .operating_system
                            .get_or_insert_with(OperatingSystemInfo::default)
                            .dns_server = Some(info);
                    }
                }
                "AdDs" => {
                    if let Ok(info) = serde_json::from_value::<AdDsInfo>(data.clone()) {
                        collection
                            .operating_system
                            .get_or_insert_with(OperatingSystemInfo::default)
                            .ad_ds = Some(info);
                    }
                }
                _ => {
                    debug!(collector = result.name.as_str(), "Unknown collector type");
                }
            }
        }
    }
}

impl Default for CollectionOrchestrator {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    struct TestCollector;

    #[async_trait]
    impl Collector for TestCollector {
        fn name(&self) -> &'static str {
            "Test"
        }

        async fn collect(&self) -> anyhow::Result<Value> {
            Ok(json!({"test": "data"}))
        }

        fn data_type(&self) -> &'static str {
            "test"
        }
    }

    #[tokio::test]
    async fn test_orchestrator_adds_collectors() {
        let mut orch = CollectionOrchestrator::new();
        orch.add_collector(Box::new(TestCollector));
        assert_eq!(orch.collectors.len(), 1);
    }

    #[tokio::test]
    async fn test_full_collection_includes_all() {
        let orch = CollectionOrchestrator::full_collection();
        assert_eq!(orch.collectors.len(), 19); // All our collectors
    }
}
