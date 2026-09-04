use crate::collectors::Collector;
use crate::models::{AdDsInfo, AdSiteInfo, DomainControllerInfo, FsmoRoles};
use crate::windows_utils::wmi::WmiHelper;
use anyhow::Result;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::collections::HashSet;
use tracing::debug;

pub struct AdDsCollector;

#[async_trait]
impl Collector for AdDsCollector {
    fn name(&self) -> &'static str {
        "AdDs"
    }

    fn data_type(&self) -> &'static str {
        "ad_ds"
    }

    fn estimated_duration_ms(&self) -> u64 {
        3500
    }

    fn requires_admin(&self) -> bool {
        true
    }

    async fn collect(&self) -> Result<Value> {
        debug!("Starting AdDs collection");

        if !WmiHelper::is_windows_server_build().await.unwrap_or(false) {
            return Ok(json!(AdDsInfo::default()));
        }

        let ntds_service = WmiHelper::get_service("NTDS").await.ok().flatten();
        if ntds_service.is_none() {
            return Ok(json!(AdDsInfo::default()));
        }

        let mut out = AdDsInfo {
            is_domain_controller: true,
            fsmo_roles: FsmoRoles::default(),
            ..Default::default()
        };

        let computer = WmiHelper::get_computer_info().await.ok();
        out.domain_name = computer
            .as_ref()
            .and_then(|v| v.get("Domain"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        // Domain controller discovery from NT domain inventory.
        let nt_domains = WmiHelper::query_values(
            "SELECT DomainControllerAddress, ClientSiteName, DomainName FROM Win32_NTDomain",
        )
        .await
        .unwrap_or_default();
        let mut seen_dc = HashSet::new();
        let mut seen_sites = HashSet::new();

        for row in nt_domains {
            let dc = row
                .get("DomainControllerAddress")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .trim_start_matches("\\\\")
                .to_string();
            let site_name = row
                .get("ClientSiteName")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());

            if !dc.is_empty() && seen_dc.insert(dc.clone()) {
                out.domain_controllers.push(DomainControllerInfo {
                    name: dc,
                    site: site_name.clone(),
                    is_global_catalog: None,
                });
            }

            if let Some(site) = site_name {
                if !site.is_empty() && seen_sites.insert(site.clone()) {
                    out.sites.push(AdSiteInfo {
                        name: site,
                        subnets: Vec::new(),
                    });
                }
            }
        }

        debug!("AdDs collection completed");
        Ok(json!(out))
    }
}
