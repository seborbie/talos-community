use anyhow::{anyhow, Result};
use registry::{Data, Hive, Security};
use serde_json::Value;
use std::collections::HashMap;
use tracing::{debug, trace};

/// Helper for Windows Registry access
pub struct RegistryHelper;

impl RegistryHelper {
    fn parse_hive(hive: &str) -> Result<Hive> {
        match hive {
            "HKLM" | "HKEY_LOCAL_MACHINE" => Ok(Hive::LocalMachine),
            "HKCU" | "HKEY_CURRENT_USER" => Ok(Hive::CurrentUser),
            "HKCR" | "HKEY_CLASSES_ROOT" => Ok(Hive::ClassesRoot),
            "HKU" | "HKEY_USERS" => Ok(Hive::Users),
            "HKCC" | "HKEY_CURRENT_CONFIG" => Ok(Hive::CurrentConfig),
            _ => Err(anyhow!("Unsupported registry hive: {}", hive)),
        }
    }

    fn open_key(hive: &str, key: &str) -> Result<registry::RegKey> {
        let hive = Self::parse_hive(hive)?;
        hive.open(key, Security::Read)
            .map_err(|e| anyhow!("Failed to open key {}\\{}: {}", hive, key, e))
    }

    fn data_to_json(data: &Data) -> Value {
        match data {
            Data::None => Value::Null,
            Data::String(s) | Data::ExpandString(s) => Value::String(s.to_string_lossy()),
            Data::Binary(b) => Value::Array(
                b.iter()
                    .map(|v| Value::Number((*v as u64).into()))
                    .collect(),
            ),
            Data::U32(v) | Data::U32BE(v) => Value::Number((*v as u64).into()),
            Data::U64(v) => Value::Number((*v).into()),
            Data::MultiString(v) => Value::Array(
                v.iter()
                    .map(|s| Value::String(s.to_string_lossy()))
                    .collect(),
            ),
            _ => Value::String(data.to_string()),
        }
    }

    /// Read a string value from registry
    pub fn read_string(hive: &str, key: &str, value: &str) -> Result<Option<String>> {
        trace!(hive, key, value, "Reading registry string");
        let regkey = match Self::open_key(hive, key) {
            Ok(k) => k,
            Err(_) => return Ok(None),
        };

        let data = match regkey.value(value) {
            Ok(v) => v,
            Err(_) => return Ok(None),
        };

        let out = match data {
            Data::String(s) | Data::ExpandString(s) => Some(s.to_string_lossy()),
            Data::U32(v) => Some(v.to_string()),
            Data::U32BE(v) => Some(v.to_string()),
            Data::U64(v) => Some(v.to_string()),
            Data::MultiString(v) => Some(
                v.iter()
                    .map(|s| s.to_string_lossy())
                    .collect::<Vec<_>>()
                    .join(";"),
            ),
            _ => Some(data.to_string()),
        };

        Ok(out.filter(|s| !s.is_empty() && s != "null"))
    }

    /// Read a DWORD value from registry
    pub fn read_dword(hive: &str, key: &str, value: &str) -> Result<Option<u32>> {
        trace!(hive, key, value, "Reading registry DWORD");

        if let Some(s) = Self::read_string(hive, key, value)? {
            s.parse::<u32>()
                .map(Some)
                .map_err(|e| anyhow!("Failed to parse DWORD: {}", e))
        } else {
            Ok(None)
        }
    }

    /// Read a QWORD value from registry
    pub fn read_qword(hive: &str, key: &str, value: &str) -> Result<Option<u64>> {
        trace!(hive, key, value, "Reading registry QWORD");

        if let Some(s) = Self::read_string(hive, key, value)? {
            s.parse::<u64>()
                .map(Some)
                .map_err(|e| anyhow!("Failed to parse QWORD: {}", e))
        } else {
            Ok(None)
        }
    }

    /// Read a binary value from registry
    pub fn read_binary(hive: &str, key: &str, value: &str) -> Result<Option<Vec<u8>>> {
        trace!(hive, key, value, "Reading registry binary");
        let regkey = match Self::open_key(hive, key) {
            Ok(k) => k,
            Err(_) => return Ok(None),
        };
        match regkey.value(value) {
            Ok(Data::Binary(v)) => Ok(Some(v)),
            Ok(_) => Ok(None),
            Err(_) => Ok(None),
        }
    }

    /// List subkeys of a registry key
    pub fn enum_subkeys(hive: &str, key: &str) -> Result<Vec<String>> {
        trace!(hive, key, "Enumerating registry subkeys");
        let regkey = Self::open_key(hive, key)?;
        let mut subkeys = Vec::new();
        for k in regkey.keys().flatten() {
            subkeys.push(k.to_string());
        }

        debug!(hive, key, count = subkeys.len(), "Found subkeys");
        Ok(subkeys)
    }

    /// List values of a registry key
    pub fn enum_values(hive: &str, key: &str) -> Result<HashMap<String, Value>> {
        trace!(hive, key, "Enumerating registry values");
        let regkey = Self::open_key(hive, key)?;
        let mut values = HashMap::new();

        for v in regkey.values().flatten() {
            let name = v.name().to_string_lossy();
            values.insert(name, Self::data_to_json(v.data()));
        }

        debug!(hive, key, count = values.len(), "Found values");
        Ok(values)
    }

    /// Check if a registry key exists
    pub fn key_exists(hive: &str, key: &str) -> bool {
        Self::open_key(hive, key).is_ok()
    }

    /// Get installed programs from registry
    pub fn get_installed_programs() -> Result<Vec<HashMap<String, Value>>> {
        debug!("Reading installed programs from registry");

        let mut programs = Vec::new();

        // 64-bit programs: HKLM (machine) and HKCU (current user) each have their own Uninstall key
        let keys_64 = [
            (
                r"HKLM",
                r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall",
            ),
            (
                r"HKCU",
                r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall",
            ),
        ];

        for (hive, path) in &keys_64 {
            if let Ok(subkeys) = Self::enum_subkeys(hive, path) {
                for subkey in subkeys {
                    if let Ok(values) = Self::enum_values(hive, &format!("{}\\{}", path, subkey)) {
                        if let Some(Value::String(name)) = values.get("DisplayName") {
                            if !name.is_empty() {
                                let mut prog = values;
                                prog.insert(
                                    "RegistryKey".to_string(),
                                    Value::String(format!("{}\\{}\\{}", hive, path, subkey)),
                                );
                                prog.insert(
                                    "Architecture".to_string(),
                                    Value::String("64-bit".to_string()),
                                );
                                programs.push(prog);
                            }
                        }
                    }
                }
            }
        }

        // 32-bit programs on 64-bit Windows
        let key_32 = r"HKLM\SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall";
        if let Ok(subkeys) = Self::enum_subkeys("HKLM", &key_32[5..]) {
            for subkey in subkeys {
                if let Ok(values) =
                    Self::enum_values("HKLM", &format!("{}\\{}", &key_32[5..], subkey))
                {
                    if let Some(Value::String(name)) = values.get("DisplayName") {
                        if !name.is_empty() {
                            let mut prog = values;
                            prog.insert(
                                "RegistryKey".to_string(),
                                Value::String(format!("HKLM\\{}\\{}", &key_32[5..], subkey)),
                            );
                            prog.insert(
                                "Architecture".to_string(),
                                Value::String("32-bit".to_string()),
                            );
                            programs.push(prog);
                        }
                    }
                }
            }
        }

        debug!(count = programs.len(), "Found installed programs");
        Ok(programs)
    }

    /// Get startup items from registry
    pub fn get_startup_items() -> Result<Vec<HashMap<String, Value>>> {
        debug!("Reading startup items from registry");

        let mut items = Vec::new();
        let locations = [
            ("HKLM", r"SOFTWARE\Microsoft\Windows\CurrentVersion\Run"),
            (
                "HKLM",
                r"SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Run",
            ),
            ("HKCU", r"SOFTWARE\Microsoft\Windows\CurrentVersion\Run"),
        ];

        for (hive, key) in &locations {
            if let Ok(values) = Self::enum_values(hive, key) {
                for (name, value) in values {
                    if name != "PSPath" && name != "PSParentPath" && name != "PSChildName" {
                        let mut item = HashMap::new();
                        item.insert("Name".to_string(), Value::String(name));
                        item.insert("Command".to_string(), value);
                        item.insert(
                            "Location".to_string(),
                            Value::String(format!("{}\\{}", hive, key)),
                        );
                        items.push(item);
                    }
                }
            }
        }

        Ok(items)
    }

    /// Get Windows version info from registry
    pub fn get_windows_version() -> Result<HashMap<String, Value>> {
        debug!("Reading Windows version from registry");

        let mut info = HashMap::new();

        if let Some(v) = Self::read_string(
            "HKLM",
            r"SOFTWARE\Microsoft\Windows NT\CurrentVersion",
            "ProductName",
        )? {
            info.insert("ProductName".to_string(), Value::String(v));
        }
        if let Some(v) = Self::read_string(
            "HKLM",
            r"SOFTWARE\Microsoft\Windows NT\CurrentVersion",
            "DisplayVersion",
        )? {
            info.insert("DisplayVersion".to_string(), Value::String(v));
        }
        if let Some(v) = Self::read_string(
            "HKLM",
            r"SOFTWARE\Microsoft\Windows NT\CurrentVersion",
            "ReleaseId",
        )? {
            info.insert("ReleaseId".to_string(), Value::String(v));
        }
        if let Some(v) = Self::read_string(
            "HKLM",
            r"SOFTWARE\Microsoft\Windows NT\CurrentVersion",
            "CurrentBuild",
        )? {
            info.insert("CurrentBuild".to_string(), Value::String(v));
        }
        if let Some(v) = Self::read_dword(
            "HKLM",
            r"SOFTWARE\Microsoft\Windows NT\CurrentVersion",
            "UBR",
        )? {
            info.insert("UBR".to_string(), Value::Number(v.into()));
        }
        if let Some(v) = Self::read_string(
            "HKLM",
            r"SOFTWARE\Microsoft\Windows NT\CurrentVersion",
            "EditionID",
        )? {
            info.insert("EditionID".to_string(), Value::String(v));
        }
        if let Some(v) = Self::read_string(
            "HKLM",
            r"SOFTWARE\Microsoft\Windows NT\CurrentVersion",
            "CompositionEditionID",
        )? {
            info.insert("CompositionEditionID".to_string(), Value::String(v));
        }
        if let Some(v) = Self::read_string(
            "HKLM",
            r"SOFTWARE\Microsoft\Windows NT\CurrentVersion",
            "InstallationType",
        )? {
            info.insert("InstallationType".to_string(), Value::String(v));
        }

        Ok(info)
    }

    /// Get Office 365/Click-to-Run info from registry
    pub fn get_office_c2r_info() -> Result<HashMap<String, Value>> {
        debug!("Reading Office Click-to-Run info from registry");

        let mut info = HashMap::new();
        let key = r"SOFTWARE\Microsoft\Office\ClickToRun";

        if !Self::key_exists("HKLM", key) {
            return Ok(info);
        }

        // Read Configuration
        if let Ok(values) = Self::enum_values("HKLM", &format!("{}\\Configuration", key)) {
            for (k, v) in values {
                info.insert(format!("C2R_Config_{}", k), v);
            }
        }

        // Read product info
        if let Ok(values) = Self::enum_values("HKLM", key) {
            for (k, v) in values {
                info.insert(format!("C2R_{}", k), v);
            }
        }

        // Check for specific Office applications
        let office_paths = [
            r"SOFTWARE\Microsoft\Windows\CurrentVersion\App Paths\outlook.exe",
            r"SOFTWARE\Microsoft\Windows\CurrentVersion\App Paths\winword.exe",
            r"SOFTWARE\Microsoft\Windows\CurrentVersion\App Paths\excel.exe",
            r"SOFTWARE\Microsoft\Windows\CurrentVersion\App Paths\powerpnt.exe",
            r"SOFTWARE\Microsoft\Windows\CurrentVersion\App Paths\msaccess.exe",
            r"SOFTWARE\Microsoft\Windows\CurrentVersion\App Paths\teams.exe",
        ];

        for path in &office_paths {
            let app_name = path
                .split('\\')
                .next_back()
                .unwrap_or("")
                .replace(".exe", "");
            if let Some(v) = Self::read_string("HKLM", path, "Path")? {
                info.insert(format!("App_{}_Path", app_name), Value::String(v));
            }
        }

        Ok(info)
    }

    /// Get OneDrive info from registry
    pub fn get_onedrive_info() -> Result<HashMap<String, Value>> {
        debug!("Reading OneDrive info from registry");

        let mut info = HashMap::new();

        // Check for OneDrive installation
        let onedrive_key = r"SOFTWARE\Microsoft\OneDrive";
        if Self::key_exists("HKLM", onedrive_key) {
            if let Some(v) = Self::read_string("HKLM", onedrive_key, "Version")? {
                info.insert("MachineVersion".to_string(), Value::String(v));
            }
            if let Some(v) = Self::read_string("HKLM", onedrive_key, "InstallPath")? {
                info.insert("MachineInstallPath".to_string(), Value::String(v));
            }
            info.insert("PerMachineInstall".to_string(), Value::Bool(true));
        }

        // User-specific OneDrive info
        let user_key = r"SOFTWARE\Microsoft\OneDrive";
        if let Some(v) = Self::read_string("HKCU", user_key, "Version")? {
            info.insert("UserVersion".to_string(), Value::String(v));
        }
        if let Some(v) = Self::read_string("HKCU", user_key, "UserFolder")? {
            info.insert("UserFolder".to_string(), Value::String(v));
        }
        if let Some(v) = Self::read_string("HKCU", user_key, "Business")? {
            info.insert("BusinessAccount".to_string(), Value::String(v));
        }

        // KFB (Known Folder Backup) status
        let kfb_key = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Explorer\User Shell Folders";
        if let Some(v) = Self::read_string("HKCU", kfb_key, "Desktop")? {
            info.insert("KFB_Desktop".to_string(), Value::String(v));
        }
        if let Some(v) = Self::read_string("HKCU", kfb_key, "Personal")? {
            info.insert("KFB_Documents".to_string(), Value::String(v));
        }
        if let Some(v) = Self::read_string("HKCU", kfb_key, "My Pictures")? {
            info.insert("KFB_Pictures".to_string(), Value::String(v));
        }

        Ok(info)
    }

    /// Get Entra ID (Azure AD) join info from registry
    pub fn get_entra_join_info() -> Result<HashMap<String, Value>> {
        debug!("Reading Entra join info from registry");

        let mut info = HashMap::new();
        let join_key = r"SYSTEM\CurrentControlSet\Control\CloudDomainJoin\JoinInfo";

        if Self::key_exists("HKLM", join_key) {
            if let Ok(subkeys) = Self::enum_subkeys("HKLM", join_key) {
                for subkey in subkeys {
                    if let Ok(values) =
                        Self::enum_values("HKLM", &format!("{}\\{}", join_key, subkey))
                    {
                        for (k, v) in values {
                            info.insert(format!("Join_{}_{}", subkey, k), v);
                        }
                    }
                }
            }
        }

        // Also check tenant info
        let tenant_key = r"SOFTWARE\Microsoft\Windows\CurrentVersion\CDJ\AAD";
        if let Ok(values) = Self::enum_values("HKLM", tenant_key) {
            for (k, v) in values {
                info.insert(format!("Tenant_{}", k), v);
            }
        }

        Ok(info)
    }

    /// Get Intune enrollment info from registry
    pub fn get_intune_info() -> Result<HashMap<String, Value>> {
        debug!("Reading Intune enrollment info from registry");

        let mut info = HashMap::new();
        let intune_key = r"SOFTWARE\Microsoft\Provisioning\OMADM\Accounts";

        if Self::key_exists("HKLM", intune_key) {
            let mut has_account_subkeys = false;
            if let Ok(subkeys) = Self::enum_subkeys("HKLM", intune_key) {
                has_account_subkeys = !subkeys.is_empty();
                for subkey in subkeys {
                    let account_key = format!("{}\\{}", intune_key, subkey);
                    if let Ok(values) = Self::enum_values("HKLM", &account_key) {
                        for (k, v) in values {
                            info.insert(format!("Account_{}_{}", subkey, k), v);
                        }
                    }
                }
            }

            // The Accounts key exists on non-enrolled devices too.
            // Treat as enrolled only when at least one account subkey exists.
            info.insert("IsEnrolled".to_string(), Value::Bool(has_account_subkeys));
        } else {
            info.insert("IsEnrolled".to_string(), Value::Bool(false));
        }

        // Device management info
        let dm_key = r"SOFTWARE\Microsoft\Enrollments\Status";
        if let Ok(values) = Self::enum_values("HKLM", dm_key) {
            for (k, v) in values {
                info.insert(format!("DM_{}", k), v);
            }
        }

        Ok(info)
    }

    /// Get Windows Defender info from registry
    pub fn get_defender_info() -> Result<HashMap<String, Value>> {
        debug!("Reading Windows Defender info from registry");

        let mut info = HashMap::new();
        let defender_key = r"SOFTWARE\Microsoft\Windows Defender";

        if !Self::key_exists("HKLM", defender_key) {
            return Ok(info);
        }

        // Check if Defender is disabled
        if let Some(v) = Self::read_dword("HKLM", defender_key, "DisableAntiSpyware")? {
            info.insert("DisableAntiSpyware".to_string(), Value::Bool(v != 0));
        }

        // Read real-time protection status
        let rt_key = r"SOFTWARE\Microsoft\Windows Defender\Real-Time Protection";
        if Self::key_exists("HKLM", rt_key) {
            if let Some(v) = Self::read_dword("HKLM", rt_key, "DisableRealtimeMonitoring")? {
                info.insert("DisableRealtimeMonitoring".to_string(), Value::Bool(v != 0));
            }
        }

        // Get signature version info
        let sig_key = r"SOFTWARE\Microsoft\Windows Defender\Signature Updates";
        if Self::key_exists("HKLM", sig_key) {
            if let Some(v) = Self::read_string("HKLM", sig_key, "AVSignatureVersion")? {
                info.insert("AVSignatureVersion".to_string(), Value::String(v));
            }
            if let Some(v) = Self::read_string("HKLM", sig_key, "AVSignatureVersion")? {
                info.insert("SignatureVersion".to_string(), Value::String(v));
            }
            if let Some(v) = Self::read_string("HKLM", sig_key, "SignatureUpdateTime")? {
                info.insert("SignatureUpdateTime".to_string(), Value::String(v));
            }
        }

        Ok(info)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_key_exists() {
        // This should exist on all Windows systems
        assert!(RegistryHelper::key_exists("HKLM", r"SOFTWARE\Microsoft"));
    }

    #[test]
    fn test_read_nonexistent_key() {
        let result = RegistryHelper::read_string("HKLM", r"SOFTWARE\NonExistentKey12345", "Value");
        assert!(result.is_ok());
        assert!(result.unwrap().is_none());
    }
}
