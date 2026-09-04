use crate::collectors::Collector;
use crate::models::{ServiceInfo, ServicesInfo};
use crate::windows_utils::wmi::WmiHelper;
use anyhow::Result;
use async_trait::async_trait;
use serde_json::{json, Value};
use tracing::debug;

pub struct ServicesCollector;

#[async_trait]
impl Collector for ServicesCollector {
    fn name(&self) -> &'static str {
        "Services"
    }

    fn data_type(&self) -> &'static str {
        "services"
    }

    fn estimated_duration_ms(&self) -> u64 {
        3000
    }

    fn requires_admin(&self) -> bool {
        false
    }

    async fn collect(&self) -> Result<Value> {
        debug!("Starting Services collection");

        let mut services = ServicesInfo::default();

        // Get services from WMI
        let wmi_services = WmiHelper::get_services().await.unwrap_or_default();

        for svc in &wmi_services {
            let name = svc
                .get("Name")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .unwrap_or_default();

            let display_name = svc
                .get("DisplayName")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .unwrap_or_default();

            let status = svc
                .get("State")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .unwrap_or_else(|| "Unknown".to_string());

            let start_type = svc
                .get("StartMode")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .unwrap_or_else(|| "Unknown".to_string());

            let account = svc
                .get("StartName")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .unwrap_or_else(|| "LocalSystem".to_string());

            let process_id = svc
                .get("ProcessId")
                .and_then(|v| v.as_u64())
                .map(|p| p as u32);

            let description = svc
                .get("Description")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .unwrap_or_default();

            let path = svc
                .get("PathName")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .unwrap_or_default();

            // Determine if critical service
            let is_critical = self.is_critical_service(&name);

            // Update counts
            services.total_count += 1;
            if status == "Running" {
                services.running_count += 1;
            } else {
                services.stopped_count += 1;
            }
            if start_type == "Auto" {
                services.auto_start_count += 1;
            }

            let service_info = ServiceInfo {
                name: name.clone(),
                display_name,
                status: status.clone(),
                start_type: start_type.clone(),
                account,
                process_id,
                can_stop: svc
                    .get("AcceptStop")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false),
                can_pause: svc
                    .get("AcceptPause")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false),
                description,
                path,
                is_critical,
            };

            services.services.push(service_info.clone());

            if is_critical {
                services.critical_services.push(service_info);
            }
        }

        // Sort services by name
        services.services.sort_by(|a, b| a.name.cmp(&b.name));

        debug!("Services collection completed");

        Ok(json!(services))
    }
}

impl ServicesCollector {
    fn is_critical_service(&self, name: &str) -> bool {
        let critical_services = [
            " RpcSs",             // RPC
            " PlugPlay",          // Plug and Play
            " DcomLaunch",        // DCOM Server Process Launcher
            " Lsass",             // Local Security Authority
            " services",          // Service Control Manager
            " wininit",           // Windows Startup Application
            " csrss",             // Client Server Runtime
            " smss",              // Session Manager
            " System",            // System
            " Registry",          // Registry
            " Dnscache",          // DNS Client
            " Dhcp",              // DHCP Client
            " NlaSvc",            // Network Location Awareness
            " netprofm",          // Network List Service
            " MpsSvc",            // Windows Firewall
            " BFE",               // Base Filtering Engine
            " EventLog",          // Windows Event Log
            " LanmanServer",      // Server
            " LanmanWorkstation", // Workstation
            " TermService",       // Remote Desktop Services
            " wuauserv",          // Windows Update
        ];

        critical_services
            .iter()
            .any(|cs| name.eq_ignore_ascii_case(cs.trim()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_services_collector_name() {
        let collector = ServicesCollector;
        assert_eq!(collector.name(), "Services");
    }

    #[test]
    fn test_critical_service_detection() {
        let collector = ServicesCollector;
        assert!(collector.is_critical_service("RpcSs"));
        assert!(collector.is_critical_service("lsass"));
        assert!(!collector.is_critical_service("SomeRandomService"));
    }
}
