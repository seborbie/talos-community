use crate::collectors::Collector;
use crate::models::{DnsServerInfo, DnsZoneInfo};
use crate::windows_utils::wmi::WmiHelper;
use anyhow::Result;
use async_trait::async_trait;
use serde_json::{json, Value};
use tracing::debug;

pub struct DnsServerCollector;

#[async_trait]
impl Collector for DnsServerCollector {
    fn name(&self) -> &'static str {
        "DnsServer"
    }

    fn data_type(&self) -> &'static str {
        "dns_server"
    }

    fn estimated_duration_ms(&self) -> u64 {
        3500
    }

    fn requires_admin(&self) -> bool {
        true
    }

    async fn collect(&self) -> Result<Value> {
        debug!("Starting DnsServer collection");

        if !WmiHelper::is_windows_server_build().await.unwrap_or(false) {
            return Ok(json!(DnsServerInfo::default()));
        }

        if WmiHelper::get_service("DNS").await.ok().flatten().is_none() {
            return Ok(json!(DnsServerInfo::default()));
        }

        let mut out = DnsServerInfo {
            installed: true,
            ..Default::default()
        };

        let server = WmiHelper::query_values_in_namespace(
            "ROOT\\MicrosoftDNS",
            "SELECT Name FROM MicrosoftDNS_Server",
        )
        .await
        .unwrap_or_default();
        if let Some(name) = server
            .first()
            .and_then(|v| v.get("Name"))
            .and_then(|v| v.as_str())
        {
            out.server_name = Some(name.to_string());
        }

        let zones = WmiHelper::query_values_in_namespace(
            "ROOT\\MicrosoftDNS",
            "SELECT Name, ZoneType, AllowUpdate FROM MicrosoftDNS_Zone",
        )
        .await
        .unwrap_or_default();

        for z in zones {
            let zone_type = z
                .get("ZoneType")
                .and_then(|v| v.as_u64())
                .map(|v| match v {
                    0 => "Cache",
                    1 => "Primary",
                    2 => "Secondary",
                    3 => "Stub",
                    _ => "Unknown",
                })
                .map(|s| s.to_string());
            let dynamic_update = z
                .get("AllowUpdate")
                .and_then(|v| v.as_u64())
                .map(|v| match v {
                    0 => "Disabled",
                    1 => "NonSecureAndSecure",
                    2 => "SecureOnly",
                    _ => "Unknown",
                })
                .map(|s| s.to_string());

            out.zones.push(DnsZoneInfo {
                name: z
                    .get("Name")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string(),
                zone_type,
                dynamic_update,
            });
        }

        debug!("DnsServer collection completed");
        Ok(json!(out))
    }
}
