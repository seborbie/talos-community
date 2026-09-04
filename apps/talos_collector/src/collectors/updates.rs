use crate::collectors::Collector;
use crate::models::{PendingUpdate, UpdateHistoryEntry, UpdatesInfo, WindowsUpdateDetails};
use crate::windows_utils::{registry::RegistryHelper, windows_update, wmi::WmiHelper};
use anyhow::Result;
use async_trait::async_trait;
use serde_json::{json, Value};
use tracing::debug;

pub struct UpdatesCollector;

#[async_trait]
impl Collector for UpdatesCollector {
    fn name(&self) -> &'static str {
        "Updates"
    }

    fn data_type(&self) -> &'static str {
        "updates"
    }

    fn estimated_duration_ms(&self) -> u64 {
        5000 // Can take longer due to Windows Update API
    }

    fn requires_admin(&self) -> bool {
        false
    }

    async fn collect(&self) -> Result<Value> {
        debug!("Starting Updates collection");

        let mut updates = UpdatesInfo::default();
        let wu_status = windows_update::get_windows_update_status().await.ok();

        updates.windows_update = self
            .collect_windows_update_details(wu_status.as_ref())
            .await?;

        // Get additional update info
        if let Some(wu_status) = wu_status.as_ref() {
            updates.optional_updates = wu_status.optional_updates.clone();
            updates.driver_updates = wu_status.driver_updates.clone();

            // Parse update history
            for entry in &wu_status.history {
                updates.update_history.push(UpdateHistoryEntry {
                    date: entry.date,
                    title: entry.title.clone(),
                    operation: entry.operation.clone(),
                    result: entry.result.clone(),
                    kb_article: self.extract_kb_from_title(&entry.title),
                    hresult: None,
                });
            }
        }
        updates
            .todo_data_collection
            .push("TODO: update_history[].hresult is not implemented yet.".to_string());

        debug!("Updates collection completed");

        Ok(json!(updates))
    }
}

impl UpdatesCollector {
    async fn collect_windows_update_details(
        &self,
        wu_status: Option<&windows_update::NativeWindowsUpdateStatus>,
    ) -> Result<WindowsUpdateDetails> {
        let mut details = WindowsUpdateDetails::default();

        // Reuse the already-fetched WU status from collect().
        if let Some(wu_status) = wu_status {
            // Pending updates
            for update in &wu_status.pending_updates {
                details.pending_updates.push(PendingUpdate {
                    title: update.title.clone(),
                    description: update.title.clone(),
                    kb_article: update.kb.clone(),
                    is_mandatory: update.is_mandatory,
                    size_bytes: update.size,
                    requires_reboot: update.reboot_required,
                });
            }
            details.pending_count = details.pending_updates.len() as u32;
            details.pending_reboot = details
                .pending_updates
                .iter()
                .any(|update| update.requires_reboot);

            // Last scan time
            details.last_scan = wu_status.last_search_time;

            // Most recent successful install timestamp from update history.
            details.last_successful_install = wu_status
                .history
                .iter()
                .filter(|h| h.operation == "Installation" && h.result.starts_with("Succeeded"))
                .map(|h| h.date)
                .max();
        }

        if details.last_scan.is_none() {
            details.last_scan = self.get_last_scan_from_registry();
        }

        // Get settings from registry
        let au_key = r"SOFTWARE\Policies\Microsoft\Windows\WindowsUpdate\AU";
        let au_key_default = r"SOFTWARE\Microsoft\Windows\CurrentVersion\WindowsUpdate\Auto Update";

        // AUOptions: 2 = Notify, 3 = Auto download/notify install, 4 = Auto download/auto install
        let au_options = RegistryHelper::read_dword("HKLM", au_key, "AUOptions")
            .ok()
            .flatten()
            .or_else(|| {
                RegistryHelper::read_dword("HKLM", au_key_default, "AUOptions")
                    .ok()
                    .flatten()
            });

        details.au_options = match au_options {
            Some(2) => "Notify".to_string(),
            Some(3) => "AutoDownloadNotifyInstall".to_string(),
            Some(4) => "AutoDownloadAutoInstall".to_string(),
            Some(5) => "AllowUserConfig".to_string(),
            _ => "Unknown".to_string(),
        };

        // Check for WSUS server
        if let Ok(Some(server)) = RegistryHelper::read_string(
            "HKLM",
            r"SOFTWARE\Policies\Microsoft\Windows\WindowsUpdate",
            "WUServer",
        ) {
            details.wu_server = Some(server.clone());
            details.use_wu_server = !server.is_empty();
        }

        // Check if Windows Update service is running
        let wuauserv = WmiHelper::get_service("wuauserv")
            .await
            .ok()
            .flatten()
            .and_then(|v| {
                v.get("State")
                    .and_then(|s| s.as_str())
                    .map(|s| s.to_string())
            });

        details.service_status = wuauserv.unwrap_or_else(|| "Unknown".to_string());

        Ok(details)
    }

    fn extract_kb_from_title(&self, title: &str) -> Option<String> {
        let start = title.find("KB")?;
        let digits: String = title[start + 2..]
            .chars()
            .take_while(|c| c.is_ascii_digit())
            .collect();
        if digits.is_empty() {
            return None;
        }
        Some(format!("KB{digits}"))
    }

    fn get_last_scan_from_registry(&self) -> Option<chrono::DateTime<chrono::Utc>> {
        let key =
            r"SOFTWARE\Microsoft\Windows\CurrentVersion\WindowsUpdate\Auto Update\Results\Detect";
        let raw = RegistryHelper::read_string("HKLM", key, "LastSuccessTime")
            .ok()
            .flatten()?;

        if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(&raw) {
            return Some(dt.with_timezone(&chrono::Utc));
        }
        if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(&raw, "%Y-%m-%d %H:%M:%S") {
            return Some(chrono::DateTime::from_naive_utc_and_offset(dt, chrono::Utc));
        }
        if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(&raw, "%m/%d/%Y %I:%M:%S %p") {
            return Some(chrono::DateTime::from_naive_utc_and_offset(dt, chrono::Utc));
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_updates_collector_name() {
        let collector = UpdatesCollector;
        assert_eq!(collector.name(), "Updates");
    }

    #[test]
    fn test_extract_kb_from_title() {
        let collector = UpdatesCollector;
        assert_eq!(
            collector.extract_kb_from_title("Security Update for Windows (KB5001234)"),
            Some("KB5001234".to_string())
        );
        assert_eq!(
            collector.extract_kb_from_title("Cumulative Update for Windows 11 (KB5034441)"),
            Some("KB5034441".to_string())
        );
        assert_eq!(
            collector.extract_kb_from_title("Some random update without KB"),
            None
        );
    }
}
