use crate::collectors::Collector;
use crate::models::{
    InstalledProgram, SoftwareInfo, StartupItem, WindowsFeature, WindowsUpdateSummary,
};
use crate::windows_utils::{registry::RegistryHelper, windows_update, wmi::WmiHelper};
use anyhow::Result;
use async_trait::async_trait;
use chrono::Utc;
use futures::future::try_join4;
use serde_json::{json, Value};
use std::collections::HashMap;
use tracing::debug;

pub struct SoftwareCollector;

#[async_trait]
impl Collector for SoftwareCollector {
    fn name(&self) -> &'static str {
        "Software"
    }

    fn data_type(&self) -> &'static str {
        "software"
    }

    fn estimated_duration_ms(&self) -> u64 {
        3000
    }

    fn requires_admin(&self) -> bool {
        false // Some apps may not be visible without admin, but many will
    }

    async fn collect(&self) -> Result<Value> {
        debug!("Starting Software collection");

        let mut software = SoftwareInfo::default();
        let (installed_programs, updates_and_features, startup_items, dot_net_versions) =
            try_join4(
                self.collect_installed_programs(),
                self.collect_windows_updates_and_features(),
                self.collect_startup_items(),
                self.collect_dotnet_versions(),
            )
            .await?;
        let (windows_updates, features) = updates_and_features;

        software.installed_programs = installed_programs;
        software.windows_updates = windows_updates;
        software.features = features;
        software.startup_items = startup_items;
        software.dot_net_versions = dot_net_versions;

        debug!("Software collection completed");

        Ok(json!(software))
    }
}

impl SoftwareCollector {
    async fn collect_installed_programs(&self) -> Result<Vec<InstalledProgram>> {
        let mut programs = Vec::new();

        // Get from registry
        let reg_programs = tokio::task::spawn_blocking(RegistryHelper::get_installed_programs)
            .await
            .map_err(|e| anyhow::anyhow!("Registry task join failed: {}", e))??;

        for prog in reg_programs {
            let name = prog
                .get("DisplayName")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .unwrap_or_default();

            if name.is_empty() {
                continue;
            }

            let publisher = prog
                .get("Publisher")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .unwrap_or_default();

            let version = prog
                .get("DisplayVersion")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .unwrap_or_default();

            let install_date = prog
                .get("InstallDate")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());

            let size_bytes = prog
                .get("EstimatedSize")
                .and_then(|v| v.as_u64())
                .map(|s| s * 1024); // Size is in KB

            let source = if prog.get("WindowsInstaller").and_then(|v| v.as_u64()) == Some(1) {
                "msi"
            } else if prog
                .get("UninstallString")
                .and_then(|v| v.as_str())
                .map(|s| s.contains(".exe"))
                .unwrap_or(false)
            {
                "exe"
            } else {
                "other"
            }
            .to_string();

            let location = prog
                .get("InstallLocation")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .filter(|s| !s.is_empty());

            let uninstall_string = prog
                .get("UninstallString")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());

            let is_64_bit = prog
                .get("Architecture")
                .and_then(|v| v.as_str())
                .map(|s| s == "64-bit")
                .unwrap_or(false);

            programs.push(InstalledProgram {
                name,
                publisher,
                version,
                install_date,
                size_bytes,
                source,
                location,
                uninstall_string,
                is_64_bit,
            });
        }

        // Deduplicate: same product can appear in 64-bit and 32-bit views (e.g. HKLM + WOW6432).
        // Keep one authoritative entry per (name, version, publisher); prefer 64-bit, then has location, then larger size.
        let mut by_key: HashMap<String, InstalledProgram> = HashMap::new();
        for prog in programs {
            let key = format!(
                "{}|||{}|||{}",
                prog.name.to_lowercase().trim(),
                prog.version,
                prog.publisher.to_lowercase().trim()
            );
            let replace = match by_key.get(&key) {
                None => true,
                Some(existing) => {
                    if prog.is_64_bit && !existing.is_64_bit {
                        true
                    } else if !prog.is_64_bit && existing.is_64_bit {
                        false
                    } else if prog.location.is_some() && existing.location.is_none() {
                        true
                    } else if prog.location.is_none() && existing.location.is_some() {
                        false
                    } else {
                        prog.size_bytes > existing.size_bytes
                    }
                }
            };
            if replace {
                by_key.insert(key, prog);
            }
        }
        let mut programs: Vec<InstalledProgram> = by_key.into_values().collect();

        // Sort by name
        programs.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));

        debug!(
            "Found {} installed programs (after deduplication)",
            programs.len()
        );
        Ok(programs)
    }

    async fn collect_windows_updates_and_features(
        &self,
    ) -> Result<(WindowsUpdateSummary, Vec<WindowsFeature>)> {
        let mut updates = WindowsUpdateSummary::default();
        let mut by_name: HashMap<String, WindowsFeature> = HashMap::new();

        let (installed_updates, wmi_features) =
            match WmiHelper::get_installed_updates_and_optional_features().await {
                Ok(data) => data,
                Err(error) => {
                    // Fall back to separate calls to keep behavior stable if batch querying fails.
                    debug!(
                        %error,
                        "batched WMI update/feature query failed, falling back to individual calls"
                    );
                    (
                        WmiHelper::get_installed_updates().await.unwrap_or_default(),
                        WmiHelper::get_optional_features().await.unwrap_or_default(),
                    )
                }
            };

        updates.installed_count = installed_updates.len() as u32;

        // Find most recent install date
        let mut latest_date: Option<chrono::DateTime<Utc>> = None;
        for update in &installed_updates {
            if let Some(date_str) = update.get("InstalledOn").and_then(|v| v.as_str()) {
                // Parse date string like "1/15/2024"
                if let Ok(date) = chrono::NaiveDate::parse_from_str(date_str, "%m/%d/%Y") {
                    if let Some(naive_dt) = date.and_hms_opt(0, 0, 0) {
                        let dt = chrono::DateTime::from_naive_utc_and_offset(naive_dt, Utc);
                        if latest_date.is_none_or(|latest| dt > latest) {
                            latest_date = Some(dt);
                        }
                    }
                }
            }
        }
        updates.last_install_date = latest_date;

        // Get pending updates via native update helper.
        if let Ok(native_updates) = windows_update::get_windows_update_status().await {
            updates.pending_count = native_updates.pending_update_count;
            updates.pending_reboot = native_updates
                .pending_updates
                .iter()
                .any(|u| u.reboot_required);
        }

        // Check Automatic Updates setting
        if let Ok(Some(au_options)) = RegistryHelper::read_dword(
            "HKLM",
            r"SOFTWARE\Policies\Microsoft\Windows\WindowsUpdate\AU",
            "AUOptions",
        ) {
            updates.automatic_updates_enabled = au_options >= 2;
        } else if let Ok(Some(au_options)) = RegistryHelper::read_dword(
            "HKLM",
            r"SOFTWARE\Microsoft\Windows\CurrentVersion\WindowsUpdate\Auto Update",
            "AUOptions",
        ) {
            updates.automatic_updates_enabled = au_options >= 2;
        }

        // Get update server
        if let Ok(Some(wu_server)) = RegistryHelper::read_string(
            "HKLM",
            r"SOFTWARE\Policies\Microsoft\Windows\WindowsUpdate",
            "WUServer",
        ) {
            updates.update_server = Some(wu_server);
        }

        // Get features from WMI (may contain duplicates across scopes)
        for feature in wmi_features {
            let name = feature
                .get("Name")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .unwrap_or_default();
            if name.is_empty() {
                continue;
            }
            let wf = WindowsFeature {
                name: name.clone(),
                display_name: feature
                    .get("DisplayName")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
                    .unwrap_or_default(),
                enabled: feature
                    .get("InstallState")
                    .and_then(|v| v.as_u64())
                    .map(|s| s == 1)
                    .unwrap_or(false),
                install_state: feature
                    .get("InstallState")
                    .and_then(|v| v.as_u64())
                    .map(|s| {
                        match s {
                            1 => "Enabled",
                            2 => "Disabled",
                            3 => "Absent",
                            4 => "Unknown",
                            _ => "Unknown",
                        }
                        .to_string()
                    })
                    .unwrap_or_else(|| "Unknown".to_string()),
            };
            by_name.insert(name, wf);
        }

        let mut features: Vec<WindowsFeature> = by_name.into_values().collect();
        features.sort_by(|a, b| a.name.cmp(&b.name));
        Ok((updates, features))
    }

    async fn collect_startup_items(&self) -> Result<Vec<StartupItem>> {
        let mut items = Vec::new();

        // Get from registry
        let reg_items = tokio::task::spawn_blocking(RegistryHelper::get_startup_items)
            .await
            .map_err(|e| anyhow::anyhow!("Registry task join failed: {}", e))??;

        let mut seen_startup: HashMap<String, ()> = HashMap::new();
        for item in reg_items {
            let name = item
                .get("Name")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .unwrap_or_default();

            let command = item
                .get("Command")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .unwrap_or_default();

            let location = item
                .get("Location")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .unwrap_or_default();

            // Deduplicate: same (name, command) can appear in HKLM and WOW6432Node Run
            let dedup_key = format!("{}|||{}", name.to_lowercase(), command);
            if seen_startup.insert(dedup_key, ()).is_some() {
                continue;
            }

            let user = if location.starts_with("HKLM") {
                "All Users"
            } else {
                "Current User"
            }
            .to_string();

            items.push(StartupItem {
                name,
                command,
                location,
                user,
                is_enabled: true, // Registry startup items are always enabled
            });
        }

        // Also check startup folders
        let startup_folders = vec![
            format!(
                r"{}\Microsoft\Windows\Start Menu\Programs\Startup",
                std::env::var("APPDATA").unwrap_or_default()
            ),
            format!(
                r"{}\Microsoft\Windows\Start Menu\Programs\Startup",
                std::env::var("ProgramData").unwrap_or_default()
            ),
        ];

        for folder in &startup_folders {
            if let Ok(entries) = std::fs::read_dir(folder) {
                for entry in entries.flatten() {
                    if let Ok(metadata) = entry.metadata() {
                        if metadata.is_file() || metadata.is_symlink() {
                            let name = entry
                                .file_name()
                                .to_string_lossy()
                                .trim_end_matches(".lnk")
                                .to_string();
                            let path = entry.path().to_string_lossy().to_string();
                            let dedup_key = format!("{}|||{}", name.to_lowercase(), path);
                            if seen_startup.insert(dedup_key, ()).is_some() {
                                continue;
                            }
                            items.push(StartupItem {
                                name,
                                command: path.clone(),
                                location: folder.clone(),
                                user: if folder.contains("ProgramData") {
                                    "All Users"
                                } else {
                                    "Current User"
                                }
                                .to_string(),
                                is_enabled: true,
                            });
                        }
                    }
                }
            }
        }

        Ok(items)
    }

    async fn collect_dotnet_versions(&self) -> Result<Vec<String>> {
        let mut versions = Vec::new();

        // Check registry for .NET Framework versions
        let dotnet_key = r"SOFTWARE\Microsoft\NET Framework Setup\NDP";

        if let Ok(subkeys) = RegistryHelper::enum_subkeys("HKLM", dotnet_key) {
            for subkey in subkeys {
                if subkey.starts_with("v") {
                    // Check if this version is installed
                    let version_key = format!("{}\\{}", dotnet_key, subkey);
                    if let Ok(Some(install)) =
                        RegistryHelper::read_dword("HKLM", &version_key, "Install")
                    {
                        if install != 0 {
                            // Get version details
                            let version = if let Ok(Some(v)) =
                                RegistryHelper::read_string("HKLM", &version_key, "Version")
                            {
                                format!("{} ({})", subkey, v)
                            } else {
                                subkey.clone()
                            };
                            versions.push(version);
                        }
                    }
                }
            }
        }

        // Also check for .NET Core / .NET 5+
        let dotnet_core_key = r"SOFTWARE\dotnet\Setup\InstalledVersions";
        if RegistryHelper::key_exists("HKLM", dotnet_core_key) {
            versions.push(".NET Core/.NET 5+ installed".to_string());
        }

        Ok(versions)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_software_collector_name() {
        let collector = SoftwareCollector;
        assert_eq!(collector.name(), "Software");
    }
}
