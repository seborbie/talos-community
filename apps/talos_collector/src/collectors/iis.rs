use crate::collectors::Collector;
use crate::models::{IisAppPoolInfo, IisInfo, IisSiteInfo};
use crate::windows_utils::wmi::WmiHelper;
use anyhow::Result;
use async_trait::async_trait;
use serde_json::{json, Value};
use tracing::debug;

pub struct IisCollector;

#[async_trait]
impl Collector for IisCollector {
    fn name(&self) -> &'static str {
        "IIS"
    }

    fn data_type(&self) -> &'static str {
        "iis"
    }

    fn estimated_duration_ms(&self) -> u64 {
        3000
    }

    fn requires_admin(&self) -> bool {
        true
    }

    async fn collect(&self) -> Result<Value> {
        debug!("Starting IIS collection");

        if !WmiHelper::is_windows_server_build().await.unwrap_or(false) {
            return Ok(json!(IisInfo::default()));
        }

        let mut out = IisInfo::default();
        let app_pools = WmiHelper::query_values_in_namespace(
            "ROOT\\WebAdministration",
            "SELECT Name, AutoStart, ManagedRuntimeVersion, State FROM ApplicationPool",
        )
        .await
        .unwrap_or_default();
        let sites = WmiHelper::query_values_in_namespace(
            "ROOT\\WebAdministration",
            "SELECT Name, Id, State FROM Site",
        )
        .await
        .unwrap_or_default();

        out.installed = !(app_pools.is_empty() && sites.is_empty());

        for p in app_pools {
            out.app_pools.push(IisAppPoolInfo {
                name: p
                    .get("Name")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string(),
                state: p
                    .get("State")
                    .map(map_iis_state)
                    .unwrap_or_else(|| "Unknown".to_string()),
                auto_start: p.get("AutoStart").and_then(|v| v.as_bool()),
            });
        }

        for s in sites {
            out.sites.push(IisSiteInfo {
                name: s
                    .get("Name")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string(),
                state: s
                    .get("State")
                    .map(map_iis_state)
                    .unwrap_or_else(|| "Unknown".to_string()),
                id: s.get("Id").and_then(|v| v.as_u64()).map(|v| v as u32),
            });
        }

        debug!("IIS collection completed");
        Ok(json!(out))
    }
}

fn map_iis_state(v: &Value) -> String {
    if let Some(s) = v.as_str() {
        return s.to_string();
    }
    match v.as_u64().unwrap_or(0) {
        1 => "Started".to_string(),
        2 => "Starting".to_string(),
        3 => "Stopped".to_string(),
        4 => "Stopping".to_string(),
        _ => "Unknown".to_string(),
    }
}
