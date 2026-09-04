use crate::collectors::Collector;
use crate::models::{
    CoManagementInfo, DeviceCertificate, EntraIntuneInfo, EntraJoinInfo, IntuneInfo,
};
use crate::windows_utils::{registry::RegistryHelper, wmi::WmiHelper};
use anyhow::Result;
use async_trait::async_trait;
use chrono::Utc;
use serde_json::{json, Value};
use tracing::debug;

pub struct EntraIntuneCollector;

#[async_trait]
impl Collector for EntraIntuneCollector {
    fn name(&self) -> &'static str {
        "EntraIntune"
    }

    fn data_type(&self) -> &'static str {
        "entra_intune"
    }

    fn estimated_duration_ms(&self) -> u64 {
        2000
    }

    fn requires_admin(&self) -> bool {
        false
    }

    async fn collect(&self) -> Result<Value> {
        debug!("Starting EntraIntune collection");

        let entra_intune = EntraIntuneInfo {
            entra_join: self.collect_entra_join().await?,
            intune: self.collect_intune().await?,
            co_management: self.collect_comanagement().await.ok(),
            todo_data_collection: vec![
                "TODO: device certificate store details (expiry/issuer) are not fully implemented yet."
                    .to_string(),
            ],
        };

        debug!("EntraIntune collection completed");

        Ok(json!(entra_intune))
    }
}

impl EntraIntuneCollector {
    async fn collect_entra_join(&self) -> Result<EntraJoinInfo> {
        let mut entra = EntraJoinInfo::default();

        // Check registry for join info
        let join_info = RegistryHelper::get_entra_join_info()?;

        // Determine join status from registry and available MDM details.
        entra.is_joined = !join_info.is_empty();
        entra.join_type = join_info
            .get("Join_Type")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| {
                if entra.is_joined {
                    "AzureAD".to_string()
                } else {
                    "None".to_string()
                }
            });

        entra.tenant_id = Self::find_registry_value(&join_info, &["Tenant_TenantId", "TenantId"]);
        entra.tenant_name =
            Self::find_registry_value(&join_info, &["Tenant_TenantName", "TenantName"]);
        entra.device_id = Self::find_registry_value(&join_info, &["Tenant_DeviceId", "DeviceId"]);
        entra.device_certificate_thumbprint =
            Self::find_registry_value(&join_info, &["Tenant_Thumbprint", "Thumbprint"]);
        entra.work_account_count = if entra.is_joined { 1 } else { 0 };

        // Preserve raw registry-derived status for debugging and transport.
        entra.dsregcmd_status = join_info.clone();

        // Get certificate info
        entra.certificates = self.get_device_certificates(&join_info).await?;

        Ok(entra)
    }

    async fn collect_intune(&self) -> Result<IntuneInfo> {
        let mut intune = IntuneInfo::default();

        // Check registry for Intune enrollment
        let intune_info = RegistryHelper::get_intune_info()?;

        intune.is_enrolled = intune_info
            .get("IsEnrolled")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        if intune.is_enrolled {
            // Get last sync time
            let mdm_path = r"SOFTWARE\Microsoft\Provisioning\OMADM\Accounts";
            if let Ok(subkeys) = RegistryHelper::enum_subkeys("HKLM", mdm_path) {
                for account in subkeys {
                    let account_key = format!("{}\\{}", mdm_path, account);

                    if let Ok(Some(last_sync)) = RegistryHelper::read_string(
                        "HKLM",
                        &format!("{}\\Protected", account_key),
                        "ServerLastSuccessTime",
                    ) {
                        // Parse sync time (WMI datetime format)
                        if let Some(dt) = WmiHelper::parse_wmi_datetime_str(&last_sync) {
                            intune.last_sync_time = Some(dt);
                            let minutes_since = (Utc::now() - dt).num_minutes() as u64;
                            intune.minutes_since_last_sync = Some(minutes_since);
                        }
                    }

                    // Get enrollment type
                    if let Ok(Some(enroll_type)) =
                        RegistryHelper::read_string("HKLM", &account_key, "EnrollmentType")
                    {
                        intune.enrollment_type = Some(enroll_type);
                    }

                    // Get MDM provider
                    if RegistryHelper::read_string("HKLM", &account_key, "MdmServiceUri").is_ok() {
                        intune.mdm_provider = Some("Microsoft Intune".to_string());
                    }

                    // Get pending reboot status
                    if let Ok(Some(pending)) =
                        RegistryHelper::read_dword("HKLM", &account_key, "RebootRequired")
                    {
                        intune.pending_reboot = Some(pending != 0);
                    }
                }
            }

            // Get sync status from WMI
            if let Ok(Some(mdm_detail)) = WmiHelper::get_mdm_details().await {
                // Parse MDM details
                if let Some(server_id) = mdm_detail
                    .get("ServerAssignedManagedServer")
                    .and_then(|v| v.as_str())
                {
                    intune.mdm_provider = Some(server_id.to_string());
                }

                if let Some(compliant) = mdm_detail
                    .get("ServerAssignedComplianceStatus")
                    .and_then(|v| v.as_u64())
                {
                    intune.compliance_state = Some(if compliant == 1 {
                        "Compliant".to_string()
                    } else {
                        "NonCompliant".to_string()
                    });
                }
            }

            // Get user from registry
            let device_key = r"SOFTWARE\Microsoft\Windows\CurrentVersion\CDJ\AAD";
            if let Ok(Some(user)) = RegistryHelper::read_string("HKLM", device_key, "UserEmail") {
                intune.primary_user = Some(user);
            }

            // Get device category
            if let Ok(Some(category)) = RegistryHelper::read_string(
                "HKLM",
                r"SOFTWARE\Microsoft\PolicyManager\current\device\DeviceInfo",
                "DeviceCategory",
            ) {
                intune.device_category = Some(category);
            }

            let (total_policies, failed_policies) =
                self.collect_policy_counts(r"SOFTWARE\Microsoft\PolicyManager\current");
            intune.policies_applied = Some(total_policies);
            intune.policies_failed = Some(failed_policies);
        }

        // Determine sync status based on last sync time
        if let Some(minutes) = intune.minutes_since_last_sync {
            intune.sync_status = Some(if minutes < 60 {
                "Succeeded".to_string()
            } else if minutes < 1440 {
                "Warning".to_string()
            } else {
                "Failed".to_string()
            });
        }

        Ok(intune)
    }

    async fn collect_comanagement(&self) -> Result<CoManagementInfo> {
        let mut comgmt = CoManagementInfo::default();

        // Check for co-management enrollment
        let key = r"SOFTWARE\Microsoft\Windows\CurrentVersion\CDM";
        if RegistryHelper::key_exists("HKLM", key) {
            comgmt.enabled = true;

            if let Ok(Some(workload)) =
                RegistryHelper::read_string("HKLM", key, "CoManagementWorkload")
            {
                comgmt.workload = workload;
            } else {
                comgmt.workload = "Pilot".to_string();
            }

            if let Ok(Some(auto_enroll)) =
                RegistryHelper::read_dword("HKLM", key, "AutoEnrollmentEnabled")
            {
                comgmt.auto_enrollment = auto_enroll != 0;
            }

            // Get capabilities
            let capabilities = [
                ("DeviceCompliance", "Device Compliance"),
                ("ResourceAccess", "Resource Access"),
                ("WindowsUpdateForBusiness", "Windows Update"),
                ("EndpointProtection", "Endpoint Protection"),
                ("DeviceConfiguration", "Device Configuration"),
                ("OfficeClickToRun", "Office Management"),
            ];

            for (reg_name, display_name) in &capabilities {
                if let Ok(Some(enabled)) =
                    RegistryHelper::read_dword("HKLM", key, &format!("{}WorkloadEnabled", reg_name))
                {
                    if enabled != 0 {
                        comgmt.capabilities.push(display_name.to_string());
                    }
                }
            }
        }

        Ok(comgmt)
    }

    async fn get_device_certificates(
        &self,
        join_info: &std::collections::HashMap<String, Value>,
    ) -> Result<Vec<DeviceCertificate>> {
        let mut certs = Vec::new();

        // Derive known certificate details from registry join information.
        for (key, value) in join_info {
            if !key.to_ascii_lowercase().contains("thumbprint") {
                continue;
            }
            let thumbprint = match value.as_str() {
                Some(v) if !v.is_empty() => v.to_string(),
                _ => continue,
            };
            certs.push(DeviceCertificate {
                cert_type: "Device".to_string(),
                thumbprint,
                expiry_date: None,
                issuer: String::new(),
                subject: key.to_string(),
            });
        }

        Ok(certs)
    }

    fn find_registry_value(
        data: &std::collections::HashMap<String, Value>,
        keys: &[&str],
    ) -> Option<String> {
        for key in keys {
            if let Some(value) = data.get(*key).and_then(|v| v.as_str()) {
                if !value.is_empty() {
                    return Some(value.to_string());
                }
            }
        }
        data.iter().find_map(|(k, v)| {
            if keys.iter().any(|needle| k.contains(needle)) {
                v.as_str().map(|s| s.to_string())
            } else {
                None
            }
        })
    }

    fn collect_policy_counts(&self, root_key: &str) -> (u32, u32) {
        let mut total_policies = 0u32;
        let mut failed_policies = 0u32;
        let mut pending = vec![root_key.to_string()];

        while let Some(key) = pending.pop() {
            if let Ok(values) = RegistryHelper::enum_values("HKLM", &key) {
                if values.contains_key("LastError") {
                    total_policies += 1;
                    let has_failure = values
                        .get("LastError")
                        .and_then(|v| v.as_str())
                        .and_then(|s| s.parse::<i64>().ok())
                        .map(|n| n != 0)
                        .unwrap_or(false);
                    if has_failure {
                        failed_policies += 1;
                    }
                }
            }

            if let Ok(subkeys) = RegistryHelper::enum_subkeys("HKLM", &key) {
                for subkey in subkeys {
                    pending.push(format!("{}\\{}", key, subkey));
                }
            }
        }

        (total_policies, failed_policies)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_entra_intune_collector_name() {
        let collector = EntraIntuneCollector;
        assert_eq!(collector.name(), "EntraIntune");
    }
}
