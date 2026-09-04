use crate::collectors::Collector;
use crate::models::{
    AntivirusInfo, BitLockerInfo, BitLockerVolume, FirewallInfo, FirewallProfile,
    FirewallRuleCounts, LocalGroup, LocalUser, SecurityEvent, SecurityInfo, ThirdPartyAvInfo,
    UserSecurityInfo, WindowsDefenderInfo,
};
use crate::windows_utils::{registry::RegistryHelper, wmi::WmiHelper};
use anyhow::Result;
use async_trait::async_trait;
use chrono::Utc;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use tracing::debug;

pub struct SecurityCollector;

#[async_trait]
impl Collector for SecurityCollector {
    fn name(&self) -> &'static str {
        "Security"
    }

    fn data_type(&self) -> &'static str {
        "security"
    }

    fn estimated_duration_ms(&self) -> u64 {
        3000
    }

    fn requires_admin(&self) -> bool {
        true // Most security info requires admin
    }

    async fn collect(&self) -> Result<Value> {
        debug!("Starting Security collection");

        let security = SecurityInfo {
            antivirus: self.collect_antivirus().await?,
            firewall: self.collect_firewall().await?,
            bitlocker: self.collect_bitlocker().await?,
            users: self.collect_users().await?,
            uac_enabled: self.get_uac_status().await.unwrap_or(true),
            certificates_expiring_30d: 0, // Would need certificate store access
            recent_security_events: self.collect_security_events().await.unwrap_or_default(),
            todo_data_collection: vec![
                "TODO: certificates_expiring_30d collection is not implemented yet.".to_string(),
                "TODO: users.local_users[].password_last_set is not implemented yet.".to_string(),
                "TODO: users.local_users[].last_logon is not implemented yet.".to_string(),
            ],
        };

        debug!("Security collection completed");

        Ok(json!(security))
    }
}

impl SecurityCollector {
    async fn collect_antivirus(&self) -> Result<AntivirusInfo> {
        let mut av = AntivirusInfo {
            windows_defender: self.collect_defender().await?,
            third_party: Vec::new(),
        };

        // Check for third-party AV
        if let Ok(Some(product)) = WmiHelper::get_antivirus_status().await {
            let name = product
                .get("DisplayName")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .unwrap_or_else(|| "Unknown".to_string());

            // Skip Windows Defender if it's already covered
            if !name.to_lowercase().contains("windows defender")
                && !name.to_lowercase().contains("microsoft defender")
            {
                av.third_party.push(ThirdPartyAvInfo {
                    name,
                    enabled: product
                        .get("ProductUptoDate")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false),
                    version: product
                        .get("VersionNumber")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string())
                        .unwrap_or_default(),
                    up_to_date: product
                        .get("ProductUptoDate")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false),
                    real_time_protection: product
                        .get("OnAccessScanningEnabled")
                        .and_then(|v| v.as_bool()),
                });
            }
        }

        Ok(av)
    }

    async fn collect_defender(&self) -> Result<WindowsDefenderInfo> {
        let mut defender = WindowsDefenderInfo::default();

        // Get from Defender WMI namespace when available.
        if let Ok(Some(ps_defender)) = WmiHelper::get_defender_status().await {
            defender.enabled = ps_defender
                .get("AMServiceEnabled")
                .and_then(|v| v.as_bool())
                .unwrap_or(true);

            defender.real_time_protection = ps_defender
                .get("RealTimeProtectionEnabled")
                .and_then(|v| v.as_bool())
                .unwrap_or(true);

            defender.behavior_monitoring = ps_defender
                .get("BehaviorMonitorEnabled")
                .and_then(|v| v.as_bool())
                .unwrap_or(true);

            defender.cloud_protection = ps_defender
                .get("IsPassiveMode")
                .and_then(|v| v.as_bool())
                .map(|p| !p)
                .unwrap_or(true);

            defender.antispyware_enabled = ps_defender
                .get("AntispywareEnabled")
                .and_then(|v| v.as_bool())
                .unwrap_or(true);

            defender.antivirus_enabled = ps_defender
                .get("AntivirusEnabled")
                .and_then(|v| v.as_bool())
                .unwrap_or(true);

            defender.definition_version = ps_defender
                .get("AntispywareSignatureVersion")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());

            // Parse last scan times
            if let Some(scan_time) = ps_defender.get("QuickScanAge").and_then(|v| {
                v.as_f64()
                    .or_else(|| v.as_u64().map(|n| n as f64))
                    .or_else(|| v.as_str().and_then(|s| s.parse::<f64>().ok()))
            }) {
                let hours = scan_time;
                let days_since = hours / 24.0;
                defender.quick_scan_overdue = days_since > 7.0;
            }

            if let Some(scan_time) = ps_defender.get("FullScanAge").and_then(|v| {
                v.as_f64()
                    .or_else(|| v.as_u64().map(|n| n as f64))
                    .or_else(|| v.as_str().and_then(|s| s.parse::<f64>().ok()))
            }) {
                let hours = scan_time;
                let days_since = hours / 24.0;
                defender.full_scan_overdue = days_since > 30.0;
            }

            defender.threats_detected_24h = ps_defender
                .get("ThreatsDetectedLast24Hours")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as u32;
        } else {
            // Fallback to registry
            let reg_info = RegistryHelper::get_defender_info()?;
            defender.enabled = reg_info
                .get("DisableAntiSpyware")
                .and_then(|v| v.as_bool())
                .map(|d| !d)
                .unwrap_or(true);

            defender.real_time_protection = reg_info
                .get("DisableRealtimeMonitoring")
                .and_then(|v| v.as_bool())
                .map(|d| !d)
                .unwrap_or(true);

            defender.definition_version = reg_info
                .get("SignatureVersion")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
        }

        Ok(defender)
    }

    async fn collect_firewall(&self) -> Result<FirewallInfo> {
        let mut firewall = FirewallInfo::default();

        // Get firewall product info from WMI
        if let Ok(Some(fw_product)) = WmiHelper::get_firewall_status().await {
            debug!(
                "Found firewall product: {:?}",
                fw_product.get("DisplayName")
            );
        }

        if let Ok(profiles) = WmiHelper::get_firewall_profiles_standard_cim().await {
            for profile in profiles {
                let name = profile
                    .get("Name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("Unknown")
                    .to_string();
                let enabled = profile
                    .get("Enabled")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                let default_inbound = profile
                    .get("DefaultInboundAction")
                    .and_then(|v| v.as_u64())
                    .map(|v| if v == 4 { "Block" } else { "Allow" })
                    .unwrap_or("Unknown")
                    .to_string();
                let default_outbound = profile
                    .get("DefaultOutboundAction")
                    .and_then(|v| v.as_u64())
                    .map(|v| if v == 4 { "Block" } else { "Allow" })
                    .unwrap_or("Unknown")
                    .to_string();

                match name.as_str() {
                    "Domain" => firewall.enabled.domain = enabled,
                    "Private" => firewall.enabled.private = enabled,
                    "Public" => firewall.enabled.public = enabled,
                    _ => {}
                }
                firewall.profiles.push(FirewallProfile {
                    name,
                    enabled,
                    default_inbound: default_inbound.clone(),
                    default_outbound: default_outbound.clone(),
                    stealth_mode: true,
                    inbound_count: Some(0),
                    outbound_count: Some(0),
                });
                if firewall.default_inbound.is_empty() {
                    firewall.default_inbound = default_inbound;
                }
                if firewall.default_outbound.is_empty() {
                    firewall.default_outbound = default_outbound;
                }
            }
        }

        if let Ok(rules) = WmiHelper::get_firewall_rules_standard_cim().await {
            let mut counts = FirewallRuleCounts::default();

            for rule in rules {
                let direction = rule.get("Direction").and_then(|v| v.as_u64()).unwrap_or(0);
                let profiles_mask = rule
                    .get("Profiles")
                    .and_then(|v| v.as_u64())
                    .map(|v| v as u32);

                counts.total += 1;
                if direction == 1 {
                    counts.inbound += 1;
                } else if direction == 2 {
                    counts.outbound += 1;
                }

                for profile in &mut firewall.profiles {
                    if !self.rule_matches_profile(profile.name.as_str(), profiles_mask) {
                        continue;
                    }
                    if direction == 1 {
                        profile.inbound_count = Some(profile.inbound_count.unwrap_or(0) + 1);
                    } else if direction == 2 {
                        profile.outbound_count = Some(profile.outbound_count.unwrap_or(0) + 1);
                    }
                }
            }

            firewall.rule_counts = Some(counts);
        }

        Ok(firewall)
    }

    async fn collect_bitlocker(&self) -> Result<BitLockerInfo> {
        let mut bitlocker = BitLockerInfo::default();

        if let Ok(wmi_volumes) = WmiHelper::get_bitlocker_volumes().await {
            for vol in wmi_volumes {
                let protection = read_u32(vol.get("ProtectionStatus")).unwrap_or(0);

                let drive_letter = vol
                    .get("DriveLetter")
                    .or_else(|| vol.get("DeviceID"))
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
                    .unwrap_or_default();

                let encryption_method = vol
                    .get("EncryptionMethod")
                    .and_then(|value| read_u32(Some(value)))
                    .map(|m| self.map_bitlocker_encryption_method(m))
                    .unwrap_or_else(|| "Unknown".to_string());
                let encryption_percentage = read_u32(vol.get("ConversionStatus"))
                    .or_else(|| read_u32(vol.get("EncryptionPercentage")))
                    .unwrap_or(0)
                    .min(100) as u8;
                let lock_status = match read_u32(vol.get("LockStatus")).unwrap_or(2) {
                    0 => "Unlocked",
                    1 => "Locked",
                    _ => "Unknown",
                }
                .to_string();

                bitlocker.volumes.push(BitLockerVolume {
                    drive_letter,
                    protection_status: match protection {
                        0 => "Unprotected".to_string(),
                        1 => "Protected".to_string(),
                        2 => "Unknown".to_string(),
                        _ => "Unknown".to_string(),
                    },
                    encryption_percentage,
                    lock_status,
                    recovery_key_backed_up: false,
                    encryption_method,
                });

                if protection == 1 {
                    bitlocker.enabled = true;
                }
            }
        }

        Ok(bitlocker)
    }

    async fn collect_users(&self) -> Result<UserSecurityInfo> {
        let mut users = UserSecurityInfo::default();

        // Get current user
        users.current_user = std::env::var("USERNAME").unwrap_or_else(|_| "unknown".to_string());
        // Would need more work to get SID and admin status

        let local_users = WmiHelper::get_local_users().await.unwrap_or_default();
        let local_groups = WmiHelper::get_local_groups().await.unwrap_or_default();
        let memberships = WmiHelper::get_group_memberships().await.unwrap_or_default();

        let mut group_members_by_name: HashMap<String, Vec<String>> = HashMap::new();
        for membership in memberships {
            let group_name = membership
                .get("GroupComponent")
                .and_then(|v| v.as_str())
                .and_then(Self::extract_name_from_wmi_reference)
                .unwrap_or_default();
            let member_name = membership
                .get("PartComponent")
                .and_then(|v| v.as_str())
                .and_then(Self::extract_name_from_wmi_reference)
                .unwrap_or_default();
            if !group_name.is_empty() && !member_name.is_empty() {
                group_members_by_name
                    .entry(group_name)
                    .or_default()
                    .push(member_name);
            }
        }

        let admin_members: HashSet<String> = group_members_by_name
            .get("Administrators")
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .collect();

        for group in local_groups {
            let name = group
                .get("Name")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .unwrap_or_default();
            let sid = group
                .get("SID")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .unwrap_or_default();
            let members = group_members_by_name.remove(&name).unwrap_or_default();
            users.local_groups.push(LocalGroup { name, sid, members });
        }

        for user in local_users {
            let name = user
                .get("Name")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .unwrap_or_default();
            let sid = user
                .get("SID")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .unwrap_or_default();
            let is_disabled = user
                .get("Disabled")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let password_expires = user.get("PasswordExpires").and_then(|v| v.as_bool());
            let is_admin = admin_members.contains(&name);

            let local_user = LocalUser {
                name: name.clone(),
                sid: sid.clone(),
                is_admin,
                is_disabled,
                password_expires,
                password_last_set: None,
                last_logon: None,
            };

            if name.eq_ignore_ascii_case(&users.current_user) {
                users.current_user_sid = sid.clone();
                users.is_admin = is_admin;
            }

            if is_admin {
                users.local_admins.push(local_user.clone());
            }
            users.local_users.push(local_user);
        }

        Ok(users)
    }

    async fn get_uac_status(&self) -> Result<bool> {
        // Check registry for UAC settings
        if let Ok(Some(consent_prompt)) = RegistryHelper::read_dword(
            "HKLM",
            r"SOFTWARE\Microsoft\Windows\CurrentVersion\Policies\System",
            "ConsentPromptBehaviorAdmin",
        ) {
            // 0 = No prompt, 1 = Prompt, 2 = Secure desktop, etc.
            return Ok(consent_prompt > 0);
        }

        Ok(true) // Default to assuming UAC is enabled
    }

    async fn collect_security_events(&self) -> Result<Vec<SecurityEvent>> {
        let mut events = Vec::new();

        let start = Self::wmi_datetime_hours_ago(24);
        let wql = format!(
            "SELECT Logfile, EventCode, Type, SourceName, Message, TimeGenerated FROM Win32_NTLogEvent \
             WHERE Logfile='Security' AND TimeGenerated >= '{}' \
             AND (EventCode=4624 OR EventCode=4625 OR EventCode=4648 OR EventCode=4649 OR EventCode=4672 \
             OR EventCode=4720 OR EventCode=4722 OR EventCode=4723 OR EventCode=4724 OR EventCode=4725 \
             OR EventCode=4726 OR EventCode=4738 OR EventCode=4740 OR EventCode=4767 OR EventCode=4768 \
             OR EventCode=4769 OR EventCode=4771 OR EventCode=4776 OR EventCode=4788 OR EventCode=4789 \
             OR EventCode=4964) ORDER BY TimeGenerated DESC",
            start
        );
        let event_list = WmiHelper::query_nt_events(&wql).await.unwrap_or_default();

        for event in event_list.into_iter().take(10) {
            let time = event
                .get("TimeGenerated")
                .and_then(|v| v.as_str())
                .and_then(WmiHelper::parse_wmi_datetime_str)
                .unwrap_or_else(Utc::now);
            let level = event
                .get("Type")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .unwrap_or_else(|| "Unknown".to_string());
            let message = event
                .get("Message")
                .and_then(|v| v.as_str())
                .map(|s| s.lines().next().unwrap_or(s).to_string())
                .unwrap_or_default();
            let source = event
                .get("SourceName")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .unwrap_or_default();
            let event_id = event.get("EventCode").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
            events.push(SecurityEvent {
                time,
                event_id,
                source,
                level,
                message,
            });
        }

        Ok(events)
    }

    fn map_bitlocker_encryption_method(&self, method: u32) -> String {
        match method {
            0 => "None".to_string(),
            1 => "AES_128_WITH_DIFFUSER".to_string(),
            2 => "AES_256_WITH_DIFFUSER".to_string(),
            3 => "AES_128".to_string(),
            4 => "AES_256".to_string(),
            5 => "HARDWARE_ENCRYPTION".to_string(),
            6 => "XTS_AES_128".to_string(),
            7 => "XTS_AES_256".to_string(),
            _ => "Unknown".to_string(),
        }
    }

    fn rule_matches_profile(&self, profile_name: &str, mask: Option<u32>) -> bool {
        let Some(mask) = mask else {
            return true;
        };
        if mask == 0 {
            return true;
        }
        match profile_name {
            "Domain" => (mask & 1) != 0,
            "Private" => (mask & 2) != 0,
            "Public" => (mask & 4) != 0,
            _ => true,
        }
    }

    fn extract_name_from_wmi_reference(input: &str) -> Option<String> {
        let marker = "Name=\"";
        let start = input.find(marker)? + marker.len();
        let rest = &input[start..];
        let end = rest.find('"')?;
        Some(rest[..end].to_string())
    }

    fn wmi_datetime_hours_ago(hours: i64) -> String {
        let ts = Utc::now() - chrono::Duration::hours(hours);
        ts.format("%Y%m%d%H%M%S.000000+000").to_string()
    }
}

fn read_u32(value: Option<&Value>) -> Option<u32> {
    let value = value?;
    if let Some(number) = value.as_u64() {
        return u32::try_from(number).ok();
    }
    if let Some(number) = value.as_i64() {
        return u32::try_from(number).ok();
    }
    if let Some(text) = value.as_str() {
        return text.trim().parse::<u32>().ok();
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_security_collector_name() {
        let collector = SecurityCollector;
        assert_eq!(collector.name(), "Security");
    }
}
