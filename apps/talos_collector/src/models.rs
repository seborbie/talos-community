use serde::{Deserialize, Serialize};

// ============================================================================
// System Information
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SystemInfo {
    pub hostname: String,
    pub domain: Option<String>,
    pub os: OsInfo,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub edition: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub architecture: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub locale: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub language: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub languages: Vec<String>,
    pub boot_time: Option<chrono::DateTime<chrono::Utc>>,
    pub uptime_seconds: u64,
    pub todo_data_collection: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct OsInfo {
    pub name: String,
    pub version: String,
    pub build: String,
    pub edition: String,
    pub install_date: Option<chrono::DateTime<chrono::Utc>>,
    pub architecture: String,
    pub locale: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub language: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub languages: Vec<String>,
    pub timezone: String,
    pub serial_number: Option<String>,
}

// ============================================================================
// Hardware Information
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct HardwareInfo {
    pub cpu: CpuInfo,
    pub memory: MemoryInfo,
    pub disks: Vec<DiskInfo>,
    pub gpus: Vec<GpuInfo>,
    pub network_adapters: Vec<NetworkAdapterHardware>,
    pub tpm: Option<TpmInfo>,
    pub secure_boot: Option<bool>,
    pub battery: Option<BatteryInfo>,
    pub motherboard: Option<MotherboardInfo>,
    pub todo_data_collection: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CpuInfo {
    pub brand: String,
    pub manufacturer: String,
    pub cores: u32,
    pub threads: u32,
    pub frequency_mhz: u64,
    pub architecture: String,
    pub socket: String,
    pub processor_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MemoryInfo {
    pub total_bytes: u64,
    pub available_bytes: u64,
    pub slots_total: u32,
    pub slots_used: u32,
    pub speed_mhz: u32,
    pub modules: Vec<MemoryModule>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MemoryModule {
    pub slot: String,
    pub capacity_bytes: u64,
    pub speed_mhz: u32,
    pub manufacturer: String,
    pub part_number: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DiskInfo {
    pub device_id: String,
    pub model: String,
    pub serial_number: String,
    pub interface: String,
    pub media_type: String,
    pub size_bytes: u64,
    pub smart: Option<SmartInfo>,
    pub volumes: Vec<VolumeInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SmartInfo {
    pub health_status: String,
    pub temperature_c: Option<i32>,
    pub percent_used: Option<u32>,
    pub reallocated_sectors: Option<u32>,
    pub pending_sectors: Option<u32>,
    pub power_on_hours: Option<u64>,
    pub wear_level: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct VolumeInfo {
    pub drive_letter: String,
    pub label: String,
    pub filesystem: String,
    pub total_bytes: u64,
    pub free_bytes: u64,
    pub percent_used: f32,
    pub is_bitlocker_encrypted: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GpuInfo {
    pub name: String,
    pub manufacturer: String,
    pub adapter_ram_bytes: u64,
    pub driver_version: String,
    pub driver_date: Option<String>,
    pub video_mode: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct NetworkAdapterHardware {
    pub name: String,
    pub mac_address: String,
    pub is_physical: bool,
    pub is_virtual: bool,
    pub adapter_type: String,
    pub speed_mbps: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TpmInfo {
    pub present: bool,
    pub version: String,
    pub ready: bool,
    pub enabled: bool,
    pub activated: bool,
    pub owned: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct BatteryInfo {
    pub present: bool,
    pub health_percent: Option<u32>,
    pub cycle_count: Option<u32>,
    pub design_capacity_mwh: Option<u32>,
    pub full_charge_capacity_mwh: Option<u32>,
    pub battery_status: String,
    pub estimated_runtime_minutes: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MotherboardInfo {
    pub manufacturer: String,
    pub product: String,
    pub serial_number: String,
    pub bios_version: String,
    pub bios_date: Option<String>,
}

// ============================================================================
// Network Information
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct NetworkInfo {
    pub adapters: Vec<NetworkAdapterConfig>,
    pub routing_table: Vec<RouteEntry>,
    pub dns_cache_entries: u32,
    pub active_connections: ConnectionSummary,
    pub shares: Vec<NetworkShare>,
    pub proxy: ProxyConfig,
    pub firewall_rules_count: Option<u32>,
    pub todo_data_collection: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct NetworkAdapterConfig {
    pub name: String,
    pub description: String,
    pub mac_address: String,
    pub ips: Vec<IpConfig>,
    pub gateways: Vec<String>,
    pub dns_servers: Vec<String>,
    pub dns_suffix: String,
    pub status: String,
    pub speed_mbps: Option<u64>,
    pub mtu: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct IpConfig {
    pub address: String,
    pub family: String, // IPv4 or IPv6
    pub prefix: u8,
    pub is_dhcp: bool,
    pub dhcp_server: Option<String>,
    pub lease_obtained: Option<chrono::DateTime<chrono::Utc>>,
    pub lease_expires: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RouteEntry {
    pub destination: String,
    pub mask: String,
    pub gateway: String,
    pub interface: String,
    pub metric: u32,
    pub is_persistent: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ConnectionSummary {
    pub tcp_established: u32,
    pub tcp_time_wait: u32,
    pub tcp_close_wait: u32,
    pub tcp_other: u32,
    pub udp_listeners: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct NetworkShare {
    pub name: String,
    pub path: String,
    pub share_type: String,
    pub description: String,
    pub connections: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProxyConfig {
    pub enabled: bool,
    pub auto_detect: bool,
    pub proxy_server: Option<String>,
    pub bypass_list: Vec<String>,
    pub pac_url: Option<String>,
}

// ============================================================================
// Software Information
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SoftwareInfo {
    pub installed_programs: Vec<InstalledProgram>,
    pub windows_updates: WindowsUpdateSummary,
    pub features: Vec<WindowsFeature>,
    pub startup_items: Vec<StartupItem>,
    pub dot_net_versions: Vec<String>,
    pub todo_data_collection: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub office_365: Option<Office365Info>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub onedrive: Option<OneDriveInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct InstalledProgram {
    pub name: String,
    pub publisher: String,
    pub version: String,
    pub install_date: Option<String>,
    pub size_bytes: Option<u64>,
    pub source: String, // msi, exe, store, etc.
    pub location: Option<String>,
    pub uninstall_string: Option<String>,
    pub is_64_bit: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WindowsUpdateSummary {
    pub installed_count: u32,
    pub last_install_date: Option<chrono::DateTime<chrono::Utc>>,
    pub pending_count: u32,
    pub pending_reboot: bool,
    pub automatic_updates_enabled: bool,
    pub update_server: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WindowsFeature {
    pub name: String,
    pub display_name: String,
    pub enabled: bool,
    pub install_state: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct StartupItem {
    pub name: String,
    pub command: String,
    pub location: String,
    pub user: String,
    pub is_enabled: bool,
}

// ============================================================================
// Office 365 Information
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Office365Info {
    pub installed: bool,
    pub version: Option<String>,
    pub build: Option<String>,
    pub channel: Option<String>, // Monthly Enterprise, Current, etc.
    pub applications: Vec<OfficeApplication>,
    pub activation_status: Option<String>,
    pub license_type: Option<String>,
    pub click_to_run: Option<bool>,
    pub update_enabled: Option<bool>,
    pub shared_computer_activation: Option<bool>,
    pub todo_data_collection: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct OfficeApplication {
    pub name: String,
    pub version: String,
    pub install_path: String,
    pub is_machine_wide: bool,
    pub last_used: Option<chrono::DateTime<chrono::Utc>>,
}

// ============================================================================
// Security Information
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SecurityInfo {
    pub antivirus: AntivirusInfo,
    pub firewall: FirewallInfo,
    pub bitlocker: BitLockerInfo,
    pub users: UserSecurityInfo,
    pub uac_enabled: bool,
    pub certificates_expiring_30d: u32,
    pub recent_security_events: Vec<SecurityEvent>,
    pub todo_data_collection: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AntivirusInfo {
    pub windows_defender: WindowsDefenderInfo,
    pub third_party: Vec<ThirdPartyAvInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WindowsDefenderInfo {
    pub enabled: bool,
    pub real_time_protection: bool,
    pub behavior_monitoring: bool,
    pub cloud_protection: bool,
    pub antispyware_enabled: bool,
    pub antivirus_enabled: bool,
    pub definition_version: Option<String>,
    pub definition_date: Option<String>,
    pub last_scan: Option<chrono::DateTime<chrono::Utc>>,
    pub last_scan_type: Option<String>,
    pub threats_detected_24h: u32,
    pub quick_scan_overdue: bool,
    pub full_scan_overdue: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ThirdPartyAvInfo {
    pub name: String,
    pub enabled: bool,
    pub version: String,
    pub up_to_date: bool,
    pub real_time_protection: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct FirewallInfo {
    pub enabled: FirewallEnabledStatus,
    pub profiles: Vec<FirewallProfile>,
    pub default_inbound: String,
    pub default_outbound: String,
    pub rule_counts: Option<FirewallRuleCounts>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct FirewallEnabledStatus {
    pub domain: bool,
    pub private: bool,
    pub public: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct FirewallProfile {
    pub name: String,
    pub enabled: bool,
    pub default_inbound: String,
    pub default_outbound: String,
    pub stealth_mode: bool,
    pub inbound_count: Option<u32>,
    pub outbound_count: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct FirewallRuleCounts {
    pub total: u32,
    pub inbound: u32,
    pub outbound: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct BitLockerInfo {
    pub enabled: bool,
    pub encryption_method: Option<String>,
    pub volumes: Vec<BitLockerVolume>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct BitLockerVolume {
    pub drive_letter: String,
    pub protection_status: String,
    pub encryption_percentage: u8,
    pub lock_status: String,
    pub recovery_key_backed_up: bool,
    pub encryption_method: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct UserSecurityInfo {
    pub current_user: String,
    pub current_user_sid: String,
    pub is_admin: bool,
    pub local_users: Vec<LocalUser>,
    pub local_admins: Vec<LocalUser>,
    pub local_groups: Vec<LocalGroup>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LocalUser {
    pub name: String,
    pub sid: String,
    pub is_admin: bool,
    pub is_disabled: bool,
    pub password_expires: Option<bool>,
    pub password_last_set: Option<chrono::DateTime<chrono::Utc>>,
    pub last_logon: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LocalGroup {
    pub name: String,
    pub sid: String,
    pub members: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SecurityEvent {
    pub time: chrono::DateTime<chrono::Utc>,
    pub event_id: u32,
    pub source: String,
    pub level: String,
    pub message: String,
}

// ============================================================================
// Services Information
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ServicesInfo {
    pub total_count: u32,
    pub running_count: u32,
    pub stopped_count: u32,
    pub auto_start_count: u32,
    pub delayed_auto_start_count: u32,
    pub services: Vec<ServiceInfo>,
    pub critical_services: Vec<ServiceInfo>,
    pub todo_data_collection: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ServiceInfo {
    pub name: String,
    pub display_name: String,
    pub status: String,
    pub start_type: String,
    pub account: String,
    pub process_id: Option<u32>,
    pub can_stop: bool,
    pub can_pause: bool,
    pub description: String,
    pub path: String,
    pub is_critical: bool,
}

// ============================================================================
// Updates Information
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct UpdatesInfo {
    pub windows_update: WindowsUpdateDetails,
    pub optional_updates: Vec<String>,
    pub driver_updates: Vec<String>,
    pub update_history: Vec<UpdateHistoryEntry>,
    pub todo_data_collection: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WindowsUpdateDetails {
    pub last_scan: Option<chrono::DateTime<chrono::Utc>>,
    pub last_successful_install: Option<chrono::DateTime<chrono::Utc>>,
    pub pending_count: u32,
    pub pending_reboot: bool,
    pub pending_updates: Vec<PendingUpdate>,
    pub service_status: String,
    pub au_options: String, // Automatic update settings
    pub wu_server: Option<String>,
    pub use_wu_server: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PendingUpdate {
    pub title: String,
    pub description: String,
    pub kb_article: Option<String>,
    pub is_mandatory: bool,
    pub size_bytes: Option<u64>,
    pub requires_reboot: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct UpdateHistoryEntry {
    pub date: chrono::DateTime<chrono::Utc>,
    pub title: String,
    pub operation: String, // Installation, Uninstallation
    pub result: String,    // Succeeded, Failed, InProgress
    pub kb_article: Option<String>,
    pub hresult: Option<i32>,
}

// ============================================================================
// Entra & Intune Information
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct EntraIntuneInfo {
    pub entra_join: EntraJoinInfo,
    pub intune: IntuneInfo,
    pub co_management: Option<CoManagementInfo>,
    pub todo_data_collection: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct EntraJoinInfo {
    pub is_joined: bool,
    pub join_type: String, // "AzureAD", "Hybrid", "None"
    pub tenant_id: Option<String>,
    pub tenant_name: Option<String>,
    pub device_id: Option<String>,
    pub device_certificate_thumbprint: Option<String>,
    pub join_date: Option<chrono::DateTime<chrono::Utc>>,
    pub transport_key_available: bool,
    pub certificates: Vec<DeviceCertificate>,
    pub work_account_count: u32,
    pub dsregcmd_status: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DeviceCertificate {
    pub cert_type: String,
    pub thumbprint: String,
    pub expiry_date: Option<chrono::DateTime<chrono::Utc>>,
    pub issuer: String,
    pub subject: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct IntuneInfo {
    pub is_enrolled: bool,
    pub enrollment_type: Option<String>, // "Manual", "Automatic", "Co-management"
    pub mdm_provider: Option<String>,
    pub last_sync_time: Option<chrono::DateTime<chrono::Utc>>,
    pub minutes_since_last_sync: Option<u64>,
    pub sync_status: Option<String>, // "Succeeded", "Failed", "Pending"
    pub compliance_state: Option<String>, // "Compliant", "NonCompliant", "Unknown"
    pub primary_user: Option<String>,
    pub device_category: Option<String>,
    pub management_certificate_expiry: Option<chrono::DateTime<chrono::Utc>>,
    pub policies_applied: Option<u32>,
    pub policies_failed: Option<u32>,
    pub pending_reboot: Option<bool>,
    pub enrollment_date: Option<chrono::DateTime<chrono::Utc>>,
    pub push_notification_received: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CoManagementInfo {
    pub enabled: bool,
    pub workload: String,
    pub auto_enrollment: bool,
    pub capabilities: Vec<String>,
}

use std::collections::HashMap;

// ============================================================================
// OneDrive Information
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct OneDriveInfo {
    pub installed: bool,
    pub version: Option<String>,
    pub client_type: Option<String>, // "PerMachine", "PerUser"
    pub account: Option<String>,
    pub tenant_id: Option<String>,
    pub sync_status: OneDriveSyncStatus,
    pub known_folder_backup: KfbInfo,
    pub sharepoint_synced_sites: Vec<SharePointSite>,
    pub storage_quota: StorageQuota,
    pub files_on_demand_enabled: bool,
    pub automatic_upload_bandwidth_managed: Option<bool>,
    pub configuration: HashMap<String, serde_json::Value>,
    pub todo_data_collection: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct OneDriveSyncStatus {
    pub total_files: u64,
    pub synced_files: u64,
    pub pending_files: u64,
    pub syncing: bool,
    pub has_errors: bool,
    pub error_count: u32,
    pub last_sync: Option<chrono::DateTime<chrono::Utc>>,
    pub errors: Vec<SyncError>,
    pub status_message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SyncError {
    pub path: String,
    pub error_code: String,
    pub error_message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct KfbInfo {
    pub enabled: bool,
    pub desktop_backed_up: bool,
    pub documents_backed_up: bool,
    pub pictures_backed_up: bool,
    pub kfb_migration_completed: bool,
    pub last_kfb_sync: Option<chrono::DateTime<chrono::Utc>>,
    pub redirection_configured: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SharePointSite {
    pub site_name: String,
    pub url: String,
    pub local_path: String,
    pub synced: bool,
    pub last_sync: Option<chrono::DateTime<chrono::Utc>>,
    pub file_count: u64,
    pub total_size_bytes: Option<u64>,
    pub sync_errors: Vec<SyncError>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct StorageQuota {
    pub total_bytes: u64,
    pub used_bytes: u64,
    pub remaining_bytes: u64,
    pub percent_used: f64,
}

// ============================================================================
// Events Summary
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct EventsSummary {
    pub summary: EventCounts,
    pub critical_errors: Vec<EventEntry>,
    pub warnings: Vec<EventEntry>,
    pub bsod_history: Vec<BsodEntry>,
    pub application_crashes: Vec<AppCrashEntry>,
    pub windows_update_errors: Vec<EventEntry>,
    pub todo_data_collection: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct EventCounts {
    pub critical_last_24h: u32,
    pub error_last_24h: u32,
    pub warning_last_24h: u32,
    pub information_last_24h: u32,
    pub critical_last_7d: u32,
    pub error_last_7d: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct EventEntry {
    pub time: chrono::DateTime<chrono::Utc>,
    pub source: String,
    pub event_id: u32,
    pub level: String,
    pub message: String,
    pub computer: String,
    pub user: Option<String>,
    pub raw_data: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct BsodEntry {
    pub time: chrono::DateTime<chrono::Utc>,
    pub bugcheck_code: String,
    pub parameter1: String,
    pub parameter2: String,
    pub parameter3: String,
    pub parameter4: String,
    pub caused_by_driver: String,
    pub crash_address: String,
    pub dump_file: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AppCrashEntry {
    pub time: chrono::DateTime<chrono::Utc>,
    pub app_name: String,
    pub app_version: String,
    pub exception_code: String,
    pub faulting_module: String,
    pub fault_offset: String,
}

// ============================================================================
// Extended Collection Information
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CertificatesInfo {
    pub stores: Vec<CertificateStoreInfo>,
    pub certificates_expiring_30d: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CertificateStoreInfo {
    pub location: String,
    pub store_name: String,
    pub certificates: Vec<CertificateInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CertificateInfo {
    pub thumbprint: String,
    pub subject: String,
    pub issuer: String,
    pub not_before: Option<chrono::DateTime<chrono::Utc>>,
    pub not_after: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ScheduledTasksInfo {
    pub tasks: Vec<ScheduledTaskInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ScheduledTaskInfo {
    pub name: String,
    pub path: String,
    pub state: String,
    pub enabled: bool,
    pub last_run_time: Option<chrono::DateTime<chrono::Utc>>,
    pub next_run_time: Option<chrono::DateTime<chrono::Utc>>,
    pub author: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SessionsInfo {
    pub sessions: Vec<UserSessionInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct UserSessionInfo {
    pub session_id: String,
    pub user: String,
    pub domain: Option<String>,
    pub logon_type: String,
    pub logon_time: Option<chrono::DateTime<chrono::Utc>>,
    pub authentication_package: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PrintersInfo {
    pub printers: Vec<PrinterInfo>,
    pub print_server: PrintServerDetails,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PrintServerDetails {
    pub ports: Vec<String>,
    pub drivers: Vec<String>,
    pub pending_jobs: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PrinterInfo {
    pub name: String,
    pub share_name: Option<String>,
    pub driver_name: String,
    pub port_name: String,
    pub status: String,
    pub is_shared: bool,
    pub job_count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct IisInfo {
    pub installed: bool,
    pub app_pools: Vec<IisAppPoolInfo>,
    pub sites: Vec<IisSiteInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct IisAppPoolInfo {
    pub name: String,
    pub state: String,
    pub auto_start: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct IisSiteInfo {
    pub name: String,
    pub state: String,
    pub id: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DhcpServerInfo {
    pub installed: bool,
    pub server_name: Option<String>,
    pub scopes: Vec<DhcpScopeInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DhcpScopeInfo {
    pub scope_id: String,
    pub name: String,
    pub state: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DnsServerInfo {
    pub installed: bool,
    pub server_name: Option<String>,
    pub zones: Vec<DnsZoneInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DnsZoneInfo {
    pub name: String,
    pub zone_type: Option<String>,
    pub dynamic_update: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AdDsInfo {
    pub is_domain_controller: bool,
    pub domain_name: Option<String>,
    pub fsmo_roles: FsmoRoles,
    pub sites: Vec<AdSiteInfo>,
    pub domain_controllers: Vec<DomainControllerInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct FsmoRoles {
    pub schema_master: Option<String>,
    pub domain_naming_master: Option<String>,
    pub pdc_emulator: Option<String>,
    pub rid_master: Option<String>,
    pub infrastructure_master: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AdSiteInfo {
    pub name: String,
    pub subnets: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DomainControllerInfo {
    pub name: String,
    pub site: Option<String>,
    pub is_global_catalog: Option<bool>,
}
