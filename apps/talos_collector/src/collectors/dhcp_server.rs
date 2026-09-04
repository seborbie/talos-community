use crate::collectors::Collector;
use crate::models::{DhcpScopeInfo, DhcpServerInfo};
use crate::windows_utils::wmi::WmiHelper;
use anyhow::Result;
use async_trait::async_trait;
use serde_json::{json, Value};
use tracing::debug;

pub struct DhcpServerCollector;

#[async_trait]
impl Collector for DhcpServerCollector {
    fn name(&self) -> &'static str {
        "DhcpServer"
    }

    fn data_type(&self) -> &'static str {
        "dhcp_server"
    }

    fn estimated_duration_ms(&self) -> u64 {
        3500
    }

    fn requires_admin(&self) -> bool {
        true
    }

    async fn collect(&self) -> Result<Value> {
        debug!("Starting DhcpServer collection");

        if !WmiHelper::is_windows_server_build().await.unwrap_or(false) {
            return Ok(json!(DhcpServerInfo::default()));
        }

        if WmiHelper::get_service("DHCPServer")
            .await
            .ok()
            .flatten()
            .is_none()
        {
            return Ok(json!(DhcpServerInfo::default()));
        }

        let mut out = DhcpServerInfo {
            installed: true,
            ..Default::default()
        };

        let server = WmiHelper::query_values_in_namespace(
            "ROOT\\Microsoft\\Windows\\DHCP",
            "SELECT ServerName FROM MSFT_DhcpServerv4",
        )
        .await
        .unwrap_or_default();
        if let Some(name) = server
            .first()
            .and_then(|v| v.get("ServerName"))
            .and_then(|v| v.as_str())
        {
            out.server_name = Some(name.to_string());
        }

        let scopes = WmiHelper::query_values_in_namespace(
            "ROOT\\Microsoft\\Windows\\DHCP",
            "SELECT ScopeId, Name, State FROM MSFT_DhcpServerv4Scope",
        )
        .await
        .unwrap_or_default();

        for scope in scopes {
            out.scopes.push(DhcpScopeInfo {
                scope_id: scope
                    .get("ScopeId")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string(),
                name: scope
                    .get("Name")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string(),
                state: scope
                    .get("State")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string()),
            });
        }

        debug!("DhcpServer collection completed");
        Ok(json!(out))
    }
}
