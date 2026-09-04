use anyhow::{Context, Result};
use serde_json::{Map, Value};
use tracing::debug;

/// Helper for WMI queries using the wmi crate directly.
pub struct WmiHelper;

impl WmiHelper {
    fn get_connection() -> Result<wmi::WMIConnection> {
        let com = wmi::COMLibrary::new()?;
        let con = wmi::WMIConnection::new(com)?;
        Ok(con)
    }

    fn get_connection_for_namespace(namespace: &str) -> Result<wmi::WMIConnection> {
        let com = wmi::COMLibrary::new()?;
        let con = wmi::WMIConnection::with_namespace_path(namespace, com)?;
        Ok(con)
    }

    /// Query WMI and return JSON objects with original property names.
    pub async fn query_values(wql: &str) -> Result<Vec<Value>> {
        debug!(wql, "Executing WMI query");
        let wql_owned = wql.to_string();
        tokio::task::spawn_blocking(move || {
            let con = Self::get_connection()?;
            let rows: Vec<Map<String, Value>> = con.raw_query(&wql_owned)?;
            Ok::<Vec<Value>, anyhow::Error>(rows.into_iter().map(Value::Object).collect())
        })
        .await
        .context("WMI query task panicked")?
    }

    /// Query WMI in a specific namespace and return JSON objects.
    pub async fn query_values_in_namespace(namespace: &str, wql: &str) -> Result<Vec<Value>> {
        debug!(namespace, wql, "Executing WMI query in namespace");
        let namespace_owned = namespace.to_string();
        let wql_owned = wql.to_string();
        tokio::task::spawn_blocking(move || {
            let con = Self::get_connection_for_namespace(&namespace_owned)?;
            let rows: Vec<Map<String, Value>> = con.raw_query(&wql_owned)?;
            Ok::<Vec<Value>, anyhow::Error>(rows.into_iter().map(Value::Object).collect())
        })
        .await
        .context("WMI namespace query task panicked")?
    }

    pub async fn get_object(wql: &str) -> Result<Option<Value>> {
        let mut results = Self::query_values(wql).await?;
        Ok(results.pop())
    }

    pub async fn get_os_info() -> Result<Value> {
        Self::get_object("SELECT * FROM Win32_OperatingSystem")
            .await?
            .context("Failed to get OS info")
    }

    pub async fn is_windows_server_build() -> Result<bool> {
        let os = Self::get_os_info().await?;
        let product_type = os.get("ProductType").and_then(|v| v.as_u64()).unwrap_or(1);
        Ok(product_type == 2 || product_type == 3)
    }

    pub async fn get_computer_info() -> Result<Value> {
        Self::get_object("SELECT * FROM Win32_ComputerSystem")
            .await?
            .context("Failed to get computer info")
    }

    pub async fn get_bios_info() -> Result<Value> {
        Self::get_object("SELECT * FROM Win32_BIOS")
            .await?
            .context("Failed to get BIOS info")
    }

    pub async fn get_processor_info() -> Result<Vec<Value>> {
        Self::query_values("SELECT * FROM Win32_Processor").await
    }

    pub async fn get_memory_info() -> Result<Vec<Value>> {
        Self::query_values("SELECT * FROM Win32_PhysicalMemory").await
    }

    pub async fn get_disk_drives() -> Result<Vec<Value>> {
        Self::query_values("SELECT * FROM Win32_DiskDrive").await
    }

    pub async fn get_logical_disks() -> Result<Vec<Value>> {
        Self::query_values("SELECT * FROM Win32_LogicalDisk WHERE DriveType=3").await
    }

    pub async fn get_network_adapters() -> Result<Vec<Value>> {
        Self::query_values("SELECT * FROM Win32_NetworkAdapter WHERE NetEnabled=True").await
    }

    pub async fn get_network_adapter_config() -> Result<Vec<Value>> {
        Self::query_values("SELECT * FROM Win32_NetworkAdapterConfiguration WHERE IPEnabled=True")
            .await
    }

    pub async fn get_gpu_info() -> Result<Vec<Value>> {
        Self::query_values("SELECT * FROM Win32_VideoController").await
    }

    pub async fn get_services() -> Result<Vec<Value>> {
        Self::query_values("SELECT * FROM Win32_Service").await
    }

    pub async fn get_service(name: &str) -> Result<Option<Value>> {
        let escaped = name.replace('\'', "''");
        let wql = format!("SELECT * FROM Win32_Service WHERE Name='{}'", escaped);
        Self::get_object(&wql).await
    }

    pub async fn get_tpm_info() -> Result<Option<Value>> {
        // TPM provider typically lives in this namespace on modern Windows builds.
        let mut rows = Self::query_values_in_namespace(
            "ROOT\\CIMV2\\Security\\MicrosoftTpm",
            "SELECT * FROM Win32_Tpm",
        )
        .await
        .unwrap_or_default();
        if !rows.is_empty() {
            return Ok(rows.pop());
        }

        // Fallback for environments exposing Win32_Tpm in default namespace.
        Self::get_object("SELECT * FROM Win32_Tpm")
            .await
            .or(Ok(None))
    }

    pub async fn get_battery_info() -> Result<Option<Value>> {
        Self::get_object("SELECT * FROM Win32_Battery")
            .await
            .or(Ok(None))
    }

    pub async fn get_baseboard_info() -> Result<Value> {
        Self::get_object("SELECT * FROM Win32_BaseBoard")
            .await?
            .context("Failed to get baseboard info")
    }

    pub async fn get_system_enclosure() -> Result<Value> {
        Self::get_object("SELECT * FROM Win32_SystemEnclosure")
            .await?
            .context("Failed to get system enclosure info")
    }

    pub async fn get_boot_config() -> Result<Value> {
        Self::get_object("SELECT * FROM Win32_BootConfiguration")
            .await?
            .context("Failed to get boot config")
    }

    pub async fn get_startup_commands() -> Result<Vec<Value>> {
        Self::query_values("SELECT * FROM Win32_StartupCommand").await
    }

    pub async fn get_page_file() -> Result<Option<Value>> {
        Self::get_object("SELECT * FROM Win32_PageFileUsage")
            .await
            .or(Ok(None))
    }

    pub async fn get_processes() -> Result<Vec<Value>> {
        Self::query_values("SELECT * FROM Win32_Process").await
    }

    pub async fn get_processes_by_name(name: &str) -> Result<Vec<Value>> {
        let escaped = name.replace('\'', "''");
        let wql = format!("SELECT * FROM Win32_Process WHERE Name='{}'", escaped);
        Self::query_values(&wql).await
    }

    pub async fn get_local_users() -> Result<Vec<Value>> {
        Self::query_values("SELECT * FROM Win32_UserAccount WHERE LocalAccount=True").await
    }

    pub async fn get_local_groups() -> Result<Vec<Value>> {
        Self::query_values("SELECT * FROM Win32_Group WHERE LocalAccount=True").await
    }

    pub async fn get_group_memberships() -> Result<Vec<Value>> {
        Self::query_values("SELECT * FROM Win32_GroupUser").await
    }

    pub async fn get_hotfixes() -> Result<Vec<Value>> {
        Self::query_values("SELECT * FROM Win32_QuickFixEngineering").await
    }

    pub async fn get_installed_products() -> Result<Vec<Value>> {
        Self::query_values("SELECT * FROM Win32_Product").await
    }

    pub async fn get_disk_partitions() -> Result<Vec<Value>> {
        Self::query_values("SELECT * FROM Win32_DiskPartition").await
    }

    pub async fn get_volumes() -> Result<Vec<Value>> {
        Self::query_values("SELECT * FROM Win32_LogicalDisk WHERE DriveType=3").await
    }

    pub async fn get_firewall_status() -> Result<Option<Value>> {
        Self::get_object("SELECT * FROM Win32_FirewallProduct")
            .await
            .or(Ok(None))
    }

    pub async fn get_antivirus_status() -> Result<Option<Value>> {
        Self::get_object("SELECT * FROM Win32_AntiVirusProduct")
            .await
            .or(Ok(None))
    }

    pub async fn get_defender_status() -> Result<Option<Value>> {
        let mut rows = match Self::query_values_in_namespace(
            "ROOT\\Microsoft\\Windows\\Defender",
            "SELECT * FROM MSFT_MpComputerStatus",
        )
        .await
        {
            Ok(rows) => rows,
            Err(_) => Vec::new(),
        };
        Ok(rows.pop())
    }

    pub async fn get_timezone() -> Result<Value> {
        Self::get_object("SELECT * FROM Win32_TimeZone")
            .await?
            .context("Failed to get timezone info")
    }

    pub async fn get_installed_updates() -> Result<Vec<Value>> {
        Self::get_hotfixes().await
    }

    pub async fn get_bitlocker_volumes() -> Result<Vec<Value>> {
        let rows = Self::query_values_in_namespace(
            "ROOT\\CIMV2\\Security\\MicrosoftVolumeEncryption",
            "SELECT * FROM Win32_EncryptableVolume",
        )
        .await
        .unwrap_or_default();
        if !rows.is_empty() {
            return Ok(rows);
        }

        Self::query_values("SELECT * FROM Win32_EncryptableVolume")
            .await
            .or(Ok(Vec::new()))
    }

    pub async fn get_smart_failure_status(device_id: &str) -> Result<Vec<Value>> {
        let escaped = device_id
            .replace('\\', "\\\\")
            .replace('&', "_")
            .replace('\'', "''");
        let wql = format!(
            "SELECT * FROM MSStorageDriver_FailurePredictStatus WHERE InstanceName LIKE '%{}%'",
            escaped
        );
        Self::query_values(&wql).await
    }

    pub async fn get_smart_failure_data() -> Result<Vec<Value>> {
        Self::query_values("SELECT * FROM MSStorageDriver_FailurePredictData").await
    }

    pub async fn get_smart_temperature_data() -> Result<Vec<Value>> {
        Self::query_values("SELECT * FROM MSStorageDriver_ATAPISmartData").await
    }

    pub async fn get_portable_battery() -> Result<Option<Value>> {
        Self::get_object("SELECT * FROM Win32_PortableBattery")
            .await
            .or(Ok(None))
    }

    pub async fn get_mdm_restrictions() -> Result<Option<Value>> {
        Self::get_object("SELECT * FROM MDM_Restrictions")
            .await
            .or(Ok(None))
    }

    pub async fn get_mdm_details() -> Result<Option<Value>> {
        let wql =
            "SELECT * FROM MDM_DevDetail_Ext01 WHERE InstanceID='Ext' AND ParentID='./DevDetail'";
        Self::get_object(wql).await.or(Ok(None))
    }

    pub async fn get_ip4_route_table() -> Result<Vec<Value>> {
        Self::query_values("SELECT * FROM Win32_IP4RouteTable").await
    }

    pub async fn get_shares() -> Result<Vec<Value>> {
        Self::query_values("SELECT * FROM Win32_Share").await
    }

    pub async fn get_dns_cache() -> Result<Vec<Value>> {
        Self::query_values("SELECT * FROM Win32_DNSCache").await
    }

    pub async fn get_firewall_profiles_standard_cim() -> Result<Vec<Value>> {
        Self::query_values_in_namespace(
            "ROOT\\StandardCimv2",
            "SELECT Name, Enabled, DefaultInboundAction, DefaultOutboundAction FROM MSFT_NetFirewallProfile",
        )
        .await
    }

    pub async fn get_firewall_rules_standard_cim() -> Result<Vec<Value>> {
        Self::query_values_in_namespace(
            "ROOT\\StandardCimv2",
            "SELECT DisplayName, Enabled, Direction, Action, Profiles FROM MSFT_NetFirewallRule",
        )
        .await
    }

    pub async fn get_logged_on_user_links() -> Result<Vec<Value>> {
        Self::query_values("SELECT * FROM Win32_LoggedOnUser").await
    }

    pub async fn get_logon_sessions() -> Result<Vec<Value>> {
        Self::query_values("SELECT * FROM Win32_LogonSession").await
    }

    pub async fn get_optional_features() -> Result<Vec<Value>> {
        Self::query_values("SELECT * FROM Win32_OptionalFeature").await
    }

    /// Fetch installed hotfixes and optional features with one WMI connection.
    pub async fn get_installed_updates_and_optional_features() -> Result<(Vec<Value>, Vec<Value>)> {
        debug!("Executing WMI batch query (Win32_QuickFixEngineering + Win32_OptionalFeature)");
        tokio::task::spawn_blocking(move || {
            let con = Self::get_connection()?;
            let installed_rows: Vec<Map<String, Value>> =
                con.raw_query("SELECT * FROM Win32_QuickFixEngineering")?;
            let feature_rows: Vec<Map<String, Value>> =
                con.raw_query("SELECT * FROM Win32_OptionalFeature")?;
            Ok::<(Vec<Value>, Vec<Value>), anyhow::Error>((
                installed_rows.into_iter().map(Value::Object).collect(),
                feature_rows.into_iter().map(Value::Object).collect(),
            ))
        })
        .await
        .context("WMI batch query task panicked")?
    }

    pub async fn query_nt_events(wql: &str) -> Result<Vec<Value>> {
        Self::query_values(wql).await
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
