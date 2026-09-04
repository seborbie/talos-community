use crate::collectors::Collector;
use crate::models::{KfbInfo, OneDriveInfo, OneDriveSyncStatus, SharePointSite, StorageQuota};
use crate::windows_utils::{registry::RegistryHelper, wmi::WmiHelper};
use anyhow::Result;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::path::Path;
use tracing::debug;

pub struct OneDriveCollector;

#[async_trait]
impl Collector for OneDriveCollector {
    fn name(&self) -> &'static str {
        "OneDrive"
    }

    fn data_type(&self) -> &'static str {
        "onedrive"
    }

    fn estimated_duration_ms(&self) -> u64 {
        2000
    }

    fn requires_admin(&self) -> bool {
        false
    }

    async fn collect(&self) -> Result<Value> {
        debug!("Starting OneDrive collection");

        let mut onedrive = OneDriveInfo::default();

        // Check if OneDrive is installed
        onedrive.installed = self.check_onedrive_installed().await;

        if onedrive.installed {
            // Get basic info from registry
            let od_info = RegistryHelper::get_onedrive_info()?;

            onedrive.version = od_info
                .get("MachineVersion")
                .and_then(|v| v.as_str())
                .or_else(|| od_info.get("UserVersion").and_then(|v| v.as_str()))
                .map(|s| s.to_string());

            onedrive.client_type = if od_info
                .get("PerMachineInstall")
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
            {
                Some("PerMachine".to_string())
            } else {
                Some("PerUser".to_string())
            };

            onedrive.account = od_info
                .get("BusinessAccount")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());

            // Get sync status
            onedrive.sync_status = self.collect_sync_status().await?;

            // Get KFB status
            onedrive.known_folder_backup = self.collect_kfb_status(&od_info)?;

            // Get SharePoint sites
            onedrive.sharepoint_synced_sites = self.collect_sharepoint_sites().await?;

            // Get storage quota
            onedrive.storage_quota = self.collect_storage_quota().await?;

            // Files On-Demand status
            onedrive.files_on_demand_enabled = self.get_files_on_demand_status().await;

            // Get additional config
            onedrive.configuration = self.collect_configuration().await?;
        }
        onedrive
            .todo_data_collection
            .push("TODO: sharepoint_synced_sites[].last_sync is not implemented yet.".to_string());
        onedrive.todo_data_collection.push(
            "TODO: sharepoint_synced_sites[].total_size_bytes is not implemented yet.".to_string(),
        );

        debug!("OneDrive collection completed");

        Ok(json!(onedrive))
    }
}

impl OneDriveCollector {
    async fn check_onedrive_installed(&self) -> bool {
        // Check registry
        let od_key = r"SOFTWARE\Microsoft\OneDrive";
        if RegistryHelper::key_exists("HKLM", od_key) || RegistryHelper::key_exists("HKCU", od_key)
        {
            return true;
        }

        // Check common paths
        let paths: Vec<String> = vec![
            r"C:\Program Files\Microsoft OneDrive\OneDrive.exe".to_string(),
            r"C:\Program Files (x86)\Microsoft OneDrive\OneDrive.exe".to_string(),
            format!(
                r"{}\Microsoft\OneDrive\OneDrive.exe",
                std::env::var("LOCALAPPDATA").unwrap_or_default()
            ),
            format!(
                r"{}\Microsoft\OneDrive\OneDrive.exe",
                std::env::var("ProgramFiles").unwrap_or_default()
            ),
        ];

        for path in &paths {
            if Path::new(path).exists() {
                return true;
            }
        }

        // Check process list via WMI as a native fallback.
        if let Ok(processes) = WmiHelper::get_processes_by_name("OneDrive.exe").await {
            return !processes.is_empty();
        }

        false
    }

    async fn collect_sync_status(&self) -> Result<OneDriveSyncStatus> {
        let mut status = OneDriveSyncStatus::default();

        status.syncing = self.is_onedrive_running().await;
        status.status_message = if status.syncing {
            "OneDrive is running".to_string()
        } else {
            "OneDrive is not running".to_string()
        };

        // Check OneDrive local cache
        let local_app_data = std::env::var("LOCALAPPDATA").unwrap_or_default();
        let onedrive_cache = format!(r"{}\Microsoft\OneDrive\logs\Business1", local_app_data);

        if Path::new(&onedrive_cache).exists() {
            status.synced_files = status.total_files; // Assume synced if cache exists
        }

        Ok(status)
    }

    fn collect_kfb_status(
        &self,
        od_info: &std::collections::HashMap<String, Value>,
    ) -> Result<KfbInfo> {
        let mut kfb = KfbInfo::default();

        // Check if KFB is enabled by looking at shell folder redirections
        let desktop_redirect = od_info
            .get("KFB_Desktop")
            .and_then(|v| v.as_str())
            .map(|s| s.contains("OneDrive"))
            .unwrap_or(false);

        let documents_redirect = od_info
            .get("KFB_Documents")
            .and_then(|v| v.as_str())
            .map(|s| s.contains("OneDrive"))
            .unwrap_or(false);

        let pictures_redirect = od_info
            .get("KFB_Pictures")
            .and_then(|v| v.as_str())
            .map(|s| s.contains("OneDrive"))
            .unwrap_or(false);

        kfb.enabled = desktop_redirect || documents_redirect || pictures_redirect;
        kfb.desktop_backed_up = desktop_redirect;
        kfb.documents_backed_up = documents_redirect;
        kfb.pictures_backed_up = pictures_redirect;

        // Check if migration is complete
        kfb.kfb_migration_completed = kfb.enabled;

        Ok(kfb)
    }

    async fn collect_sharepoint_sites(&self) -> Result<Vec<SharePointSite>> {
        let mut sites = Vec::new();

        // Look for SharePoint sync folders
        let user_profile = std::env::var("USERPROFILE").unwrap_or_default();
        let _onedrive_commercial = format!(r"{}\OneDrive - *", user_profile);

        // Expand wildcard manually
        if let Ok(entries) = std::fs::read_dir(&user_profile) {
            for entry in entries.flatten() {
                if let Ok(metadata) = entry.metadata() {
                    if metadata.is_dir() {
                        let name = entry.file_name().to_string_lossy().to_string();
                        if name.starts_with("OneDrive - ") || name.contains(" - ") {
                            let full_path = entry.path();

                            // Count files
                            let file_count = self.count_files_in_dir(&full_path);

                            sites.push(SharePointSite {
                                site_name: name.clone(),
                                url: format!(
                                    "https://{}.sharepoint.com",
                                    name.replace("OneDrive - ", "").replace(" ", "")
                                ),
                                local_path: full_path.to_string_lossy().to_string(),
                                synced: true,    // Assume synced if folder exists
                                last_sync: None, // Would need to check sync log
                                file_count,
                                total_size_bytes: None, // Would need to calculate
                                sync_errors: Vec::new(),
                            });
                        }
                    }
                }
            }
        }

        Ok(sites)
    }

    fn count_files_in_dir(&self, path: &Path) -> u64 {
        let mut count = 0u64;
        for entry in walkdir::WalkDir::new(path)
            .max_depth(2)
            .into_iter()
            .flatten()
        {
            if entry.file_type().is_file() {
                count += 1;
            }
        }
        count
    }

    async fn collect_storage_quota(&self) -> Result<StorageQuota> {
        let mut quota = StorageQuota::default();

        // Try to get quota from OneDrive API or registry
        // For now, estimate based on local OneDrive folder size
        let user_profile = std::env::var("USERPROFILE").unwrap_or_default();

        // Look for OneDrive folders
        let onedrive_paths: Vec<String> = if let Ok(entries) = std::fs::read_dir(&user_profile) {
            entries
                .flatten()
                .filter(|e| e.file_name().to_string_lossy().starts_with("OneDrive"))
                .filter_map(|e| e.path().to_str().map(|s| s.to_string()))
                .collect()
        } else {
            Vec::new()
        };

        let mut total_used: u64 = 0;
        for path in &onedrive_paths {
            total_used += self.get_folder_size(path);
        }

        quota.used_bytes = total_used;
        quota.total_bytes = 1099511627776u64; // 1TB default (personal)
        quota.remaining_bytes = quota.total_bytes.saturating_sub(quota.used_bytes);
        quota.percent_used = if quota.total_bytes > 0 {
            (quota.used_bytes as f64 / quota.total_bytes as f64) * 100.0
        } else {
            0.0
        };

        // Check if it's business account (5TB)
        let od_key = r"SOFTWARE\Microsoft\OneDrive";
        if let Ok(Some(tenant)) = RegistryHelper::read_string("HKCU", od_key, "TenantId") {
            if !tenant.is_empty() {
                quota.total_bytes = 5497558138880u64; // 5TB for business
                quota.remaining_bytes = quota.total_bytes.saturating_sub(quota.used_bytes);
                quota.percent_used = (quota.used_bytes as f64 / quota.total_bytes as f64) * 100.0;
            }
        }

        Ok(quota)
    }

    fn get_folder_size(&self, path: &str) -> u64 {
        let mut size = 0u64;
        for entry in walkdir::WalkDir::new(path).into_iter().flatten() {
            if let Ok(metadata) = entry.metadata() {
                if metadata.is_file() {
                    size += metadata.len();
                }
            }
        }
        size
    }

    async fn get_files_on_demand_status(&self) -> bool {
        // Check registry for Files On-Demand setting
        let key = r"SOFTWARE\Microsoft\OneDrive";
        if let Ok(Some(enabled)) = RegistryHelper::read_dword("HKCU", key, "FilesOnDemandEnabled") {
            return enabled != 0;
        }

        // Default to true for modern OneDrive
        true
    }

    async fn collect_configuration(&self) -> Result<std::collections::HashMap<String, Value>> {
        let mut config = std::collections::HashMap::new();

        // Read various OneDrive settings from registry
        let od_key = r"SOFTWARE\Microsoft\OneDrive";

        let settings = [
            "LastUpdateTime",
            "FirstRunExperienceShown",
            "PreventNetworkTrafficDuringBusinessHours",
            "AutomaticUploadBandwidthManaged",
        ];

        for setting in &settings {
            if let Ok(Some(value)) = RegistryHelper::read_string("HKCU", od_key, setting) {
                config.insert(setting.to_string(), Value::String(value));
            }
        }

        // Check autostart
        let autostart_key = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Run";
        let autostart = RegistryHelper::key_exists("HKCU", &format!("{}\\OneDrive", autostart_key));
        config.insert("AutoStart".to_string(), Value::Bool(autostart));

        Ok(config)
    }

    async fn is_onedrive_running(&self) -> bool {
        WmiHelper::get_processes_by_name("OneDrive.exe")
            .await
            .map(|procs| !procs.is_empty())
            .unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_onedrive_collector_name() {
        let collector = OneDriveCollector;
        assert_eq!(collector.name(), "OneDrive");
    }
}
