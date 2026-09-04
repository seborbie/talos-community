use crate::collectors::Collector;
use crate::models::{Office365Info, OfficeApplication};
use crate::windows_utils::{registry::RegistryHelper, winapi_helpers};
use anyhow::Result;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::path::Path;
use tracing::debug;

pub struct Office365Collector;

#[async_trait]
impl Collector for Office365Collector {
    fn name(&self) -> &'static str {
        "Office365"
    }

    fn data_type(&self) -> &'static str {
        "office365"
    }

    fn estimated_duration_ms(&self) -> u64 {
        1000
    }

    fn requires_admin(&self) -> bool {
        false
    }

    async fn collect(&self) -> Result<Value> {
        debug!("Starting Office365 collection");

        let mut office = Office365Info::default();

        // Check for Office installation
        let c2r_info = RegistryHelper::get_office_c2r_info()?;

        office.installed = !c2r_info.is_empty() || self.check_office_installed().await;

        if office.installed {
            office.version = c2r_info
                .get("C2R_Config_VersionToReport")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .or_else(|| {
                    c2r_info
                        .get("C2R_Config_ClientVersionToReport")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string())
                });

            office.build = c2r_info
                .get("C2R_Config_ClientVersionToReport")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());

            office.channel = c2r_info
                .get("C2R_Config_UpdateChannel")
                .and_then(|v| v.as_str())
                .map(|s| self.parse_channel(s));

            office.click_to_run = Some(true);
            office.update_enabled = c2r_info
                .get("C2R_Config_UpdatesEnabled")
                .and_then(|v| v.as_str())
                .map(|s| s == "True" || s == "1");

            office.shared_computer_activation = c2r_info
                .get("C2R_Config_SharedComputerLicensing")
                .and_then(|v| v.as_str())
                .map(|s| s == "1");

            // Detect individual applications
            office.applications = self.detect_applications(&c2r_info).await?;

            // Get activation status
            office.activation_status = self.get_activation_status().await.ok();
            office.license_type = self.get_license_type(&c2r_info);
        }
        office
            .todo_data_collection
            .push("TODO: office.applications[].last_used is not implemented yet.".to_string());

        debug!("Office365 collection completed");

        Ok(json!(office))
    }
}

impl Office365Collector {
    async fn check_office_installed(&self) -> bool {
        // Check common Office installation paths
        let office_paths = [
            r"C:\Program Files\Microsoft Office\root\Office16",
            r"C:\Program Files\Microsoft Office\Office16",
            r"C:\Program Files (x86)\Microsoft Office\root\Office16",
            r"C:\Program Files (x86)\Microsoft Office\Office16",
            r"C:\Program Files\Microsoft Office\root\Office15",
            r"C:\Program Files\Microsoft Office\Office15",
            r"C:\Program Files\Microsoft Office 15\root\office15",
        ];

        for path in &office_paths {
            if Path::new(path).exists() {
                return true;
            }
        }

        // Check for Office executables
        let executables = [
            "winword.exe",
            "excel.exe",
            "outlook.exe",
            "powerpnt.exe",
            "msaccess.exe",
        ];

        for exe in &executables {
            if self.find_in_path(exe).await {
                return true;
            }
        }

        false
    }

    async fn find_in_path(&self, executable: &str) -> bool {
        // Check if executable exists in Program Files
        let paths = [
            format!(
                r"C:\Program Files\Microsoft Office\root\Office16\{}",
                executable
            ),
            format!(
                r"C:\Program Files (x86)\Microsoft Office\root\Office16\{}",
                executable
            ),
        ];

        for path in &paths {
            if Path::new(path).exists() {
                return true;
            }
        }

        false
    }

    fn parse_channel(&self, channel_id: &str) -> String {
        match channel_id {
            "Insiders::DevMain" => "Dev Channel (Insider)".to_string(),
            "Insiders::CC" => "Beta Channel".to_string(),
            "Current" => "Current Channel".to_string(),
            "CurrentPreview" => "Current Channel (Preview)".to_string(),
            "MonthlyEnterprise" => "Monthly Enterprise Channel".to_string(),
            "SemiAnnual" => "Semi-Annual Enterprise Channel".to_string(),
            "SemiAnnualPreview" => "Semi-Annual Enterprise Channel (Preview)".to_string(),
            "Dogfood::DevMain" => "Dogfood".to_string(),
            _ => channel_id.to_string(),
        }
    }

    async fn detect_applications(
        &self,
        c2r_info: &std::collections::HashMap<String, Value>,
    ) -> Result<Vec<OfficeApplication>> {
        let mut apps = Vec::new();

        let app_configs = [
            ("Outlook", "Outlook", "outlook.exe"),
            ("Word", "Word", "winword.exe"),
            ("Excel", "Excel", "excel.exe"),
            ("PowerPoint", "PowerPoint", "powerpnt.exe"),
            ("Access", "Access", "msaccess.exe"),
            ("Publisher", "Publisher", "mspubl.exe"),
            ("OneNote", "OneNote", "onenote.exe"),
            ("Teams", "Teams", "teams.exe"),
            ("Skype for Business", "Lync", "lync.exe"),
            ("Project", "Project", "winproj.exe"),
            ("Visio", "Visio", "visio.exe"),
        ];

        for (name, registry_key, exe) in &app_configs {
            if let Some(path) = self.find_app_path(exe, registry_key).await {
                let version = self.get_app_version(&path).await;

                // Check if Teams is per-user or machine-wide
                let is_machine_wide = if *name == "Teams" {
                    path.to_lowercase().contains("program files")
                } else {
                    path.to_lowercase().contains("program files") || path.contains("root\\office16")
                };

                apps.push(OfficeApplication {
                    name: name.to_string(),
                    version: version.unwrap_or_else(|| {
                        c2r_info
                            .get(&format!("C2R_Config_{}_Version", registry_key))
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string())
                            .unwrap_or_else(|| "Unknown".to_string())
                    }),
                    install_path: path,
                    is_machine_wide,
                    last_used: None, // Would need registry access
                });
            }
        }

        Ok(apps)
    }

    async fn find_app_path(&self, exe_name: &str, registry_name: &str) -> Option<String> {
        // Try App Paths registry
        let app_path_key = format!(
            r"SOFTWARE\Microsoft\Windows\CurrentVersion\App Paths\{}",
            exe_name
        );
        if let Ok(Some(path)) = RegistryHelper::read_string("HKLM", &app_path_key, "Path") {
            return Some(path);
        }
        if let Ok(Some(path)) = RegistryHelper::read_string("HKCU", &app_path_key, "Path") {
            return Some(path);
        }

        // Try hardcoded paths
        let possible_paths = [
            format!(
                r"C:\Program Files\Microsoft Office\root\Office16\{}",
                exe_name
            ),
            format!(
                r"C:\Program Files (x86)\Microsoft Office\root\Office16\{}",
                exe_name
            ),
            format!(r"C:\Program Files\Microsoft Office\Office16\{}", exe_name),
            format!(
                r"C:\Program Files (x86)\Microsoft Office\Office16\{}",
                exe_name
            ),
            format!(
                r"C:\Program Files\Microsoft Office\root\Office15\{}",
                exe_name
            ),
            format!(
                r"C:\Program Files\WindowsApps\*{}*\{}",
                registry_name, exe_name
            ),
        ];

        for path in &possible_paths {
            // Handle wildcard in WindowsApps path
            if path.contains("WindowsApps") && path.contains('*') {
                if let Some(found) = self.find_in_windows_apps(exe_name).await {
                    return Some(found);
                }
            } else if Path::new(path).exists() {
                return Some(path.to_string());
            }
        }

        // Special handling for Teams which can be in AppData
        if exe_name == "teams.exe" {
            let teams_paths = [
                format!(
                    r"{}\Microsoft\Teams\current\teams.exe",
                    std::env::var("LOCALAPPDATA").unwrap_or_default()
                ),
                format!(
                    r"{}\Microsoft\Teams\current\teams.exe",
                    std::env::var("ProgramFiles").unwrap_or_default()
                ),
            ];
            for path in &teams_paths {
                if Path::new(path).exists() {
                    return Some(path.to_string());
                }
            }
        }

        None
    }

    async fn find_in_windows_apps(&self, exe_name: &str) -> Option<String> {
        let windows_apps = r"C:\Program Files\WindowsApps";
        if let Ok(entries) = std::fs::read_dir(windows_apps) {
            for entry in entries.flatten() {
                if let Ok(metadata) = entry.metadata() {
                    if metadata.is_dir() {
                        let path = entry.path();
                        let exe_path = path.join(exe_name);
                        if exe_path.exists() {
                            return Some(exe_path.to_string_lossy().to_string());
                        }
                    }
                }
            }
        }
        None
    }

    async fn get_app_version(&self, exe_path: &str) -> Option<String> {
        winapi_helpers::get_file_version(exe_path)
    }

    async fn get_activation_status(&self) -> Result<String> {
        // Check activation via ospp.vbs or registry
        let ospp_paths = [
            r"C:\Program Files\Microsoft Office\Office16\ospp.vbs",
            r"C:\Program Files (x86)\Microsoft Office\Office16\ospp.vbs",
            r"C:\Program Files\Microsoft Office\Office15\ospp.vbs",
        ];

        for path in &ospp_paths {
            if Path::new(path).exists() {
                let script = format!(
                    r#"cscript "{}" /dstatus 2>&1 | findstr /i "LICENSE""#,
                    path.replace('\\', "\\\\\\\\")
                );
                if let Ok(output) = tokio::process::Command::new("cmd")
                    .args(["/C", &script])
                    .stdout(std::process::Stdio::piped())
                    .stderr(std::process::Stdio::piped())
                    .output()
                    .await
                {
                    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
                    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
                    let output = format!("{}{}", stdout, stderr);
                    if output.to_lowercase().contains("licensed") {
                        return Ok("Activated".to_string());
                    } else if output.to_lowercase().contains("grace") {
                        return Ok("Grace Period".to_string());
                    }
                }
            }
        }

        // Check registry for activation status
        let key = r"SOFTWARE\Microsoft\Office\16.0\Common\Licensing";
        if RegistryHelper::key_exists("HKLM", key) {
            // Check for subscription or retail keys
            return Ok("Likely Activated".to_string());
        }

        Ok("Unknown".to_string())
    }

    fn get_license_type(
        &self,
        c2r_info: &std::collections::HashMap<String, Value>,
    ) -> Option<String> {
        // Determine license type from registry
        if c2r_info.contains_key("C2R_Config_O365HomePremRetail") {
            return Some("Microsoft 365 Personal/Family".to_string());
        }
        if c2r_info.contains_key("C2R_Config_O365BusinessRetail")
            || c2r_info.contains_key("C2R_Config_O365ProPlusRetail")
        {
            return Some("Microsoft 365 Business/Enterprise".to_string());
        }
        if c2r_info.contains_key("C2R_Config_ProPlus2019Retail")
            || c2r_info.contains_key("C2R_Config_ProPlus2019Volume")
        {
            return Some("Office 2019 Professional Plus".to_string());
        }
        if c2r_info.contains_key("C2R_Config_HomeBusiness2019Retail") {
            return Some("Office 2019 Home & Business".to_string());
        }
        if c2r_info.contains_key("C2R_Config_HomeStudent2019Retail") {
            return Some("Office 2019 Home & Student".to_string());
        }

        // Check for volume license indicators
        if c2r_info.values().any(|v| {
            v.as_str()
                .map(|s| s.to_lowercase().contains("volume"))
                .unwrap_or(false)
        }) {
            return Some("Volume License".to_string());
        }

        Some("Retail".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_office365_collector_name() {
        let collector = Office365Collector;
        assert_eq!(collector.name(), "Office365");
    }

    #[test]
    fn test_parse_channel() {
        let collector = Office365Collector;
        assert_eq!(
            collector.parse_channel("MonthlyEnterprise"),
            "Monthly Enterprise Channel"
        );
        assert_eq!(collector.parse_channel("Current"), "Current Channel");
        assert_eq!(
            collector.parse_channel("SemiAnnual"),
            "Semi-Annual Enterprise Channel"
        );
    }
}
