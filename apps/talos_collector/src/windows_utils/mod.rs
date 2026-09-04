//! Windows utility modules for data collection
//!
//! Provides wrappers and helpers for WMI, registry, and native Windows APIs.

pub mod event_log;
#[cfg(windows)]
pub mod registry;
pub mod winapi_helpers;
pub mod windows_update;
#[cfg(windows)]
pub mod wmi;

#[cfg(not(windows))]
pub mod registry {
    use anyhow::Result;
    use serde_json::Value;
    use std::collections::HashMap;

    /// Non-Windows placeholder for Windows Registry access.
    pub struct RegistryHelper;

    impl RegistryHelper {
        pub fn read_string(_hive: &str, _key: &str, _value: &str) -> Result<Option<String>> {
            Ok(None)
        }

        pub fn read_dword(_hive: &str, _key: &str, _value: &str) -> Result<Option<u32>> {
            Ok(None)
        }

        pub fn read_qword(_hive: &str, _key: &str, _value: &str) -> Result<Option<u64>> {
            Ok(None)
        }

        pub fn read_binary(_hive: &str, _key: &str, _value: &str) -> Result<Option<Vec<u8>>> {
            Ok(None)
        }

        pub fn enum_subkeys(_hive: &str, _key: &str) -> Result<Vec<String>> {
            Ok(Vec::new())
        }

        pub fn enum_values(_hive: &str, _key: &str) -> Result<HashMap<String, Value>> {
            Ok(HashMap::new())
        }

        pub fn key_exists(_hive: &str, _key: &str) -> bool {
            false
        }

        pub fn get_installed_programs() -> Result<Vec<HashMap<String, Value>>> {
            Ok(Vec::new())
        }

        pub fn get_startup_items() -> Result<Vec<HashMap<String, Value>>> {
            Ok(Vec::new())
        }

        pub fn get_windows_version() -> Result<HashMap<String, Value>> {
            Ok(HashMap::new())
        }

        pub fn get_office_c2r_info() -> Result<HashMap<String, Value>> {
            Ok(HashMap::new())
        }

        pub fn get_onedrive_info() -> Result<HashMap<String, Value>> {
            Ok(HashMap::new())
        }

        pub fn get_entra_join_info() -> Result<HashMap<String, Value>> {
            Ok(HashMap::new())
        }

        pub fn get_intune_info() -> Result<HashMap<String, Value>> {
            Ok(HashMap::new())
        }

        pub fn get_defender_info() -> Result<HashMap<String, Value>> {
            Ok(HashMap::new())
        }
    }
}

#[cfg(not(windows))]
pub mod wmi {
    use anyhow::Result;
    use serde_json::{Map, Value};

    /// Non-Windows placeholder for WMI queries.
    pub struct WmiHelper;

    impl WmiHelper {
        fn empty_object() -> Value {
            Value::Object(Map::new())
        }

        pub async fn query_values(_wql: &str) -> Result<Vec<Value>> {
            Ok(Vec::new())
        }

        pub async fn query_values_in_namespace(_namespace: &str, _wql: &str) -> Result<Vec<Value>> {
            Ok(Vec::new())
        }

        pub async fn get_object(_wql: &str) -> Result<Option<Value>> {
            Ok(None)
        }

        pub async fn get_os_info() -> Result<Value> {
            Ok(Self::empty_object())
        }

        pub async fn is_windows_server_build() -> Result<bool> {
            Ok(false)
        }

        pub async fn get_computer_info() -> Result<Value> {
            Ok(Self::empty_object())
        }

        pub async fn get_bios_info() -> Result<Value> {
            Ok(Self::empty_object())
        }

        pub async fn get_processor_info() -> Result<Vec<Value>> {
            Ok(Vec::new())
        }

        pub async fn get_memory_info() -> Result<Vec<Value>> {
            Ok(Vec::new())
        }

        pub async fn get_disk_drives() -> Result<Vec<Value>> {
            Ok(Vec::new())
        }

        pub async fn get_logical_disks() -> Result<Vec<Value>> {
            Ok(Vec::new())
        }

        pub async fn get_network_adapters() -> Result<Vec<Value>> {
            Ok(Vec::new())
        }

        pub async fn get_network_adapter_config() -> Result<Vec<Value>> {
            Ok(Vec::new())
        }

        pub async fn get_gpu_info() -> Result<Vec<Value>> {
            Ok(Vec::new())
        }

        pub async fn get_services() -> Result<Vec<Value>> {
            Ok(Vec::new())
        }

        pub async fn get_service(_name: &str) -> Result<Option<Value>> {
            Ok(None)
        }

        pub async fn get_tpm_info() -> Result<Option<Value>> {
            Ok(None)
        }

        pub async fn get_battery_info() -> Result<Option<Value>> {
            Ok(None)
        }

        pub async fn get_baseboard_info() -> Result<Value> {
            Ok(Self::empty_object())
        }

        pub async fn get_system_enclosure() -> Result<Value> {
            Ok(Self::empty_object())
        }

        pub async fn get_boot_config() -> Result<Value> {
            Ok(Self::empty_object())
        }

        pub async fn get_startup_commands() -> Result<Vec<Value>> {
            Ok(Vec::new())
        }

        pub async fn get_page_file() -> Result<Option<Value>> {
            Ok(None)
        }

        pub async fn get_processes() -> Result<Vec<Value>> {
            Ok(Vec::new())
        }

        pub async fn get_processes_by_name(_name: &str) -> Result<Vec<Value>> {
            Ok(Vec::new())
        }

        pub async fn get_local_users() -> Result<Vec<Value>> {
            Ok(Vec::new())
        }

        pub async fn get_local_groups() -> Result<Vec<Value>> {
            Ok(Vec::new())
        }

        pub async fn get_group_memberships() -> Result<Vec<Value>> {
            Ok(Vec::new())
        }

        pub async fn get_hotfixes() -> Result<Vec<Value>> {
            Ok(Vec::new())
        }

        pub async fn get_installed_products() -> Result<Vec<Value>> {
            Ok(Vec::new())
        }

        pub async fn get_disk_partitions() -> Result<Vec<Value>> {
            Ok(Vec::new())
        }

        pub async fn get_volumes() -> Result<Vec<Value>> {
            Ok(Vec::new())
        }

        pub async fn get_firewall_status() -> Result<Option<Value>> {
            Ok(None)
        }

        pub async fn get_antivirus_status() -> Result<Option<Value>> {
            Ok(None)
        }

        pub async fn get_defender_status() -> Result<Option<Value>> {
            Ok(None)
        }

        pub async fn get_timezone() -> Result<Value> {
            Ok(Self::empty_object())
        }

        pub async fn get_installed_updates() -> Result<Vec<Value>> {
            Ok(Vec::new())
        }

        pub async fn get_bitlocker_volumes() -> Result<Vec<Value>> {
            Ok(Vec::new())
        }

        pub async fn get_smart_failure_status(_device_id: &str) -> Result<Vec<Value>> {
            Ok(Vec::new())
        }

        pub async fn get_smart_failure_data() -> Result<Vec<Value>> {
            Ok(Vec::new())
        }

        pub async fn get_smart_temperature_data() -> Result<Vec<Value>> {
            Ok(Vec::new())
        }

        pub async fn get_portable_battery() -> Result<Option<Value>> {
            Ok(None)
        }

        pub async fn get_mdm_restrictions() -> Result<Option<Value>> {
            Ok(None)
        }

        pub async fn get_mdm_details() -> Result<Option<Value>> {
            Ok(None)
        }

        pub async fn get_ip4_route_table() -> Result<Vec<Value>> {
            Ok(Vec::new())
        }

        pub async fn get_shares() -> Result<Vec<Value>> {
            Ok(Vec::new())
        }

        pub async fn get_dns_cache() -> Result<Vec<Value>> {
            Ok(Vec::new())
        }

        pub async fn get_firewall_profiles_standard_cim() -> Result<Vec<Value>> {
            Ok(Vec::new())
        }

        pub async fn get_firewall_rules_standard_cim() -> Result<Vec<Value>> {
            Ok(Vec::new())
        }

        pub async fn get_logged_on_user_links() -> Result<Vec<Value>> {
            Ok(Vec::new())
        }

        pub async fn get_logon_sessions() -> Result<Vec<Value>> {
            Ok(Vec::new())
        }

        pub async fn get_optional_features() -> Result<Vec<Value>> {
            Ok(Vec::new())
        }

        pub async fn get_installed_updates_and_optional_features(
        ) -> Result<(Vec<Value>, Vec<Value>)> {
            Ok((Vec::new(), Vec::new()))
        }

        pub async fn query_nt_events(_wql: &str) -> Result<Vec<Value>> {
            Ok(Vec::new())
        }

        pub fn parse_wmi_datetime(value: &Value) -> Option<chrono::DateTime<chrono::Utc>> {
            let dt_str = value.as_str()?;
            Self::parse_wmi_datetime_str(dt_str)
        }

        pub fn parse_wmi_datetime_str(dt_str: &str) -> Option<chrono::DateTime<chrono::Utc>> {
            if dt_str.len() < 14 {
                return None;
            }
            let year: i32 = dt_str[0..4].parse().ok()?;
            let month: u32 = dt_str[4..6].parse().ok()?;
            let day: u32 = dt_str[6..8].parse().ok()?;
            let hour: u32 = dt_str[8..10].parse().ok()?;
            let minute: u32 = dt_str[10..12].parse().ok()?;
            let second: u32 = dt_str[12..14].parse().ok()?;

            chrono::NaiveDate::from_ymd_opt(year, month, day)
                .and_then(|d| d.and_hms_opt(hour, minute, second))
                .map(|naive| chrono::DateTime::from_naive_utc_and_offset(naive, chrono::Utc))
        }

        pub fn parse_wmi_size(value: &Value) -> Option<u64> {
            if let Some(n) = value.as_u64() {
                return Some(n);
            }
            if let Some(n) = value.as_i64() {
                return Some(n as u64);
            }
            if let Some(s) = value.as_str() {
                let cleaned: String = s.chars().filter(|c| c.is_ascii_digit()).collect();
                cleaned.parse().ok()
            } else {
                None
            }
        }
    }
}

pub use event_log::*;
pub use registry::RegistryHelper;
pub use winapi_helpers::*;
pub use windows_update::*;
pub use wmi::WmiHelper;
