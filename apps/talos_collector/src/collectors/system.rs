use crate::collectors::Collector;
use crate::models::SystemInfo;
use crate::windows_utils::{registry::RegistryHelper, wmi::WmiHelper};
use anyhow::Result;
use async_trait::async_trait;
use serde_json::{json, Value};
use sysinfo::System;
use tracing::debug;

pub struct SystemCollector;

#[async_trait]
impl Collector for SystemCollector {
    fn name(&self) -> &'static str {
        "System"
    }

    fn data_type(&self) -> &'static str {
        "system"
    }

    fn estimated_duration_ms(&self) -> u64 {
        500
    }

    fn requires_admin(&self) -> bool {
        false
    }

    async fn collect(&self) -> Result<Value> {
        debug!("Starting System collection");

        let mut sys = SystemInfo::default();

        // Get hostname
        sys.hostname = hostname::get()
            .map(|h| h.to_string_lossy().to_string())
            .unwrap_or_else(|_| "unknown".to_string());

        // Get OS info from registry
        let os_reg_info =
            match tokio::task::spawn_blocking(RegistryHelper::get_windows_version).await {
                Ok(Ok(info)) => info,
                Ok(Err(e)) => {
                    return Err(anyhow::anyhow!(
                        "RegistryHelper::get_windows_version failed: {}",
                        e
                    ))
                }
                Err(e) => return Err(anyhow::anyhow!("Registry task join failed: {}", e)),
            };

        // Get OS info from WMI
        let os_wmi = WmiHelper::get_os_info().await.ok();

        // Combine OS info
        sys.os.name = os_reg_info
            .get("ProductName")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .or_else(System::name)
            .unwrap_or_else(|| "Unknown".to_string());

        sys.os.version = os_reg_info
            .get("DisplayVersion")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .or_else(|| {
                os_reg_info
                    .get("ReleaseId")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
            })
            .unwrap_or_default();

        sys.os.build = if let Some(ubr) = os_reg_info.get("UBR").and_then(|v| v.as_u64()) {
            format!(
                "{}.{}",
                os_reg_info
                    .get("CurrentBuild")
                    .and_then(|v| v.as_str())
                    .unwrap_or("0"),
                ubr
            )
        } else {
            os_reg_info
                .get("CurrentBuild")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .or_else(System::os_version)
                .unwrap_or_default()
        };

        sys.os.edition = first_non_empty([
            os_reg_info.get("EditionID").and_then(|v| v.as_str()),
            os_reg_info
                .get("CompositionEditionID")
                .and_then(|v| v.as_str()),
            os_reg_info.get("InstallationType").and_then(|v| v.as_str()),
        ])
        .unwrap_or_default();

        sys.os.architecture = first_non_empty([
            os_wmi
                .as_ref()
                .and_then(|os| os.get("OSArchitecture"))
                .and_then(|v| v.as_str()),
            os_wmi
                .as_ref()
                .and_then(|os| os.get("Architecture"))
                .and_then(|v| v.as_str()),
            Some(native_architecture_label()),
        ])
        .map(normalize_architecture)
        .unwrap_or_else(|| "unknown".to_string());

        // Windows Setup /Auto Upgrade requires media to match the system default UI language.
        let mui_languages = os_wmi
            .as_ref()
            .and_then(|os| os.get("MUILanguages"))
            .map(read_string_list)
            .unwrap_or_default();
        let os_language = first_non_empty([
            crate::windows_utils::winapi_helpers::get_system_default_ui_language().as_deref(),
            os_wmi
                .as_ref()
                .and_then(|os| os.get("OSLanguage"))
                .map(wmi_language_code_to_locale)
                .as_deref(),
            mui_languages.first().map(|value| value.as_str()),
            os_wmi
                .as_ref()
                .and_then(|os| os.get("Locale"))
                .and_then(|v| v.as_str()),
            crate::windows_utils::winapi_helpers::get_system_locale().as_deref(),
        ])
        .map(normalize_locale)
        .unwrap_or_default();
        sys.os.locale = os_language.clone();
        sys.os.language = os_language;
        sys.os.languages = mui_languages;

        if let Some(tz) = crate::windows_utils::winapi_helpers::get_timezone() {
            sys.os.timezone = tz;
        }

        // Get domain info - check if domain-joined using WMI we already have
        if let Ok(computer_info) = WmiHelper::get_computer_info().await {
            if let Some(is_domain_joined) =
                computer_info.get("PartOfDomain").and_then(|v| v.as_bool())
            {
                if is_domain_joined {
                    if let Some(domain) = computer_info.get("Domain").and_then(|v| v.as_str()) {
                        sys.domain = Some(domain.to_string());
                    }
                }
            }
        }

        // Get install date
        if let Some(os_obj) = os_wmi {
            if let Some(install_date) = os_obj.get("InstallDate").and_then(|v| v.as_str()) {
                sys.os.install_date = WmiHelper::parse_wmi_datetime_str(install_date);
            }
            if let Some(serial) = os_obj.get("SerialNumber").and_then(|v| v.as_str()) {
                sys.os.serial_number = Some(serial.to_string());
            }
        }

        sys.edition = sys.os.edition.clone();
        sys.architecture = sys.os.architecture.clone();
        sys.locale = sys.os.locale.clone();
        sys.language = sys.os.language.clone();
        sys.languages = sys.os.languages.clone();

        // Get boot time and uptime
        let boot_time = System::boot_time();
        if boot_time > 0 {
            sys.boot_time = chrono::DateTime::from_timestamp(boot_time as i64, 0);
        }
        sys.uptime_seconds = System::uptime();

        debug!("System collection completed");

        Ok(json!(sys))
    }
}

fn first_non_empty<'a>(values: impl IntoIterator<Item = Option<&'a str>>) -> Option<String> {
    values
        .into_iter()
        .flatten()
        .map(str::trim)
        .find(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn native_architecture_label() -> &'static str {
    if cfg!(target_arch = "x86_64") {
        "x64"
    } else if cfg!(target_arch = "x86") {
        "x86"
    } else if cfg!(target_arch = "aarch64") {
        "arm64"
    } else {
        "unknown"
    }
}

fn normalize_architecture(value: String) -> String {
    let lower = value.trim().to_lowercase();
    if lower.contains("64") || lower == "amd64" || lower == "x64" {
        "x64".to_string()
    } else if lower.contains("32") || lower == "x86" || lower == "i386" {
        "x86".to_string()
    } else if lower.contains("arm64") || lower.contains("aarch64") {
        "arm64".to_string()
    } else {
        value.trim().to_string()
    }
}

fn normalize_locale(value: String) -> String {
    value.trim().replace('_', "-")
}

fn read_string_list(value: &Value) -> Vec<String> {
    match value {
        Value::Array(items) => items
            .iter()
            .filter_map(|item| item.as_str())
            .map(normalize_locale_from_str)
            .filter(|item| !item.is_empty())
            .collect(),
        Value::String(text) => text
            .split([';', ','])
            .map(normalize_locale_from_str)
            .filter(|item| !item.is_empty())
            .collect(),
        _ => Vec::new(),
    }
}

fn normalize_locale_from_str(value: &str) -> String {
    value.trim().replace('_', "-")
}

fn wmi_language_code_to_locale(value: &Value) -> String {
    let Some(code) = value
        .as_u64()
        .or_else(|| value.as_str()?.parse::<u64>().ok())
    else {
        return String::new();
    };
    match code {
        1033 => "en-US".to_string(),
        2057 => "en-GB".to_string(),
        3081 => "en-AU".to_string(),
        4105 => "en-CA".to_string(),
        1031 => "de-DE".to_string(),
        1036 => "fr-FR".to_string(),
        1040 => "it-IT".to_string(),
        1041 => "ja-JP".to_string(),
        1043 => "nl-NL".to_string(),
        1046 => "pt-BR".to_string(),
        2070 => "pt-PT".to_string(),
        1034 => "es-ES".to_string(),
        2058 => "es-MX".to_string(),
        _ => format!("{code}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_system_collector_name() {
        let collector = SystemCollector;
        assert_eq!(collector.name(), "System");
    }

    #[test]
    fn test_system_collector_not_admin() {
        let collector = SystemCollector;
        assert!(!collector.requires_admin());
    }
}
