#![cfg(target_os = "windows")]

use std::env;

use talos_protocol::RMM_DISPLAY_PROCESSING_MODE_ENV;
use tracing::{debug, warn};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DisplayProcessingMode {
    Legacy,
    Auto,
    Gpu,
}

impl DisplayProcessingMode {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Legacy => "legacy",
            Self::Auto => "auto",
            Self::Gpu => "modern_gpu",
        }
    }

    pub(crate) fn prefers_gpu(self) -> bool {
        matches!(self, Self::Auto | Self::Gpu)
    }

    pub(crate) fn allows_cpu_fallback(self) -> bool {
        matches!(self, Self::Auto)
    }

    pub(crate) fn is_legacy(self) -> bool {
        matches!(self, Self::Legacy)
    }
}

pub(crate) fn effective_display_processing_mode(context: &'static str) -> DisplayProcessingMode {
    resolve_display_processing_mode(context)
}

fn resolve_display_processing_mode(context: &'static str) -> DisplayProcessingMode {
    let raw = configured_display_processing_mode_value();
    match raw
        .as_deref()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "" | "auto" => DisplayProcessingMode::Auto,
        "legacy" => DisplayProcessingMode::Legacy,
        "modern_gpu" => DisplayProcessingMode::Gpu,
        "experimental" => {
            warn!(
                context,
                env_var = RMM_DISPLAY_PROCESSING_MODE_ENV,
                "experimental display processing mode is deprecated; using modern_gpu"
            );
            DisplayProcessingMode::Gpu
        }
        "modern_cpu" => {
            warn!(
                context,
                env_var = RMM_DISPLAY_PROCESSING_MODE_ENV,
                "modern_cpu display processing mode is removed; using legacy CPU capture"
            );
            DisplayProcessingMode::Legacy
        }
        other => {
            warn!(
                context,
                env_var = RMM_DISPLAY_PROCESSING_MODE_ENV,
                value = %other,
                "invalid display processing mode; falling back to auto mode"
            );
            DisplayProcessingMode::Auto
        }
    }
}

fn configured_display_processing_mode_value() -> Option<String> {
    env::var(RMM_DISPLAY_PROCESSING_MODE_ENV)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .or_else(|| {
            windows_registry_display_processing_value(
                RMM_DISPLAY_PROCESSING_MODE_ENV,
                "display processing mode",
            )
        })
}

#[cfg(not(target_os = "windows"))]
fn windows_registry_display_processing_value(
    _env_var: &'static str,
    _setting: &'static str,
) -> Option<String> {
    None
}

#[cfg(target_os = "windows")]
fn windows_registry_display_processing_value(
    env_var: &'static str,
    setting: &'static str,
) -> Option<String> {
    const SERVICE_ENV_KEY: &str = r"SYSTEM\CurrentControlSet\Services\TalosWorker";
    const SYSTEM_ENV_KEY: &str = r"SYSTEM\CurrentControlSet\Control\Session Manager\Environment";
    const USER_ENV_KEY: &str = "Environment";

    if let Some(value) = registry_env_block_value(
        winapi::um::winreg::HKEY_LOCAL_MACHINE,
        SERVICE_ENV_KEY,
        "Environment",
        env_var,
    ) {
        debug!(
            env_var = env_var,
            source = "HKLM service Environment",
            setting = setting,
            "display processing setting resolved from registry"
        );
        return Some(value);
    }

    if let Some(value) = registry_string_value(
        winapi::um::winreg::HKEY_LOCAL_MACHINE,
        SYSTEM_ENV_KEY,
        env_var,
    ) {
        debug!(
            env_var = env_var,
            source = "HKLM system Environment",
            setting = setting,
            "display processing setting resolved from registry"
        );
        return Some(value);
    }

    if let Some(value) =
        registry_string_value(winapi::um::winreg::HKEY_CURRENT_USER, USER_ENV_KEY, env_var)
    {
        debug!(
            env_var = env_var,
            source = "HKCU Environment",
            setting = setting,
            "display processing setting resolved from registry"
        );
        return Some(value);
    }

    None
}

#[cfg(target_os = "windows")]
fn registry_env_block_value(
    root: winapi::shared::minwindef::HKEY,
    subkey: &str,
    value_name: &str,
    env_name: &str,
) -> Option<String> {
    let (value_type, bytes) = registry_raw_value(root, subkey, value_name)?;
    if value_type != winapi::um::winnt::REG_MULTI_SZ && value_type != winapi::um::winnt::REG_SZ {
        return None;
    }
    let block = registry_utf16_string(&bytes);
    block
        .split('\0')
        .filter_map(|entry| entry.split_once('='))
        .find_map(|(name, value)| {
            if name.trim().eq_ignore_ascii_case(env_name) {
                let value = value.trim();
                (!value.is_empty()).then(|| value.to_string())
            } else {
                None
            }
        })
}

#[cfg(target_os = "windows")]
fn registry_string_value(
    root: winapi::shared::minwindef::HKEY,
    subkey: &str,
    value_name: &str,
) -> Option<String> {
    let (value_type, bytes) = registry_raw_value(root, subkey, value_name)?;
    if value_type != winapi::um::winnt::REG_SZ && value_type != winapi::um::winnt::REG_EXPAND_SZ {
        return None;
    }
    let value = registry_utf16_string(&bytes);
    let value = value.split('\0').next().unwrap_or_default().trim();
    (!value.is_empty()).then(|| value.to_string())
}

#[cfg(target_os = "windows")]
fn registry_raw_value(
    root: winapi::shared::minwindef::HKEY,
    subkey: &str,
    value_name: &str,
) -> Option<(u32, Vec<u8>)> {
    use std::ptr;
    use winapi::shared::minwindef::DWORD;
    use winapi::shared::winerror::ERROR_SUCCESS;
    use winapi::um::winnt::{KEY_READ, KEY_WOW64_64KEY};
    use winapi::um::winreg::{RegCloseKey, RegOpenKeyExW, RegQueryValueExW};

    let subkey_wide: Vec<u16> = subkey.encode_utf16().chain(std::iter::once(0)).collect();
    let value_wide: Vec<u16> = value_name
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let mut key = ptr::null_mut();
    let success = ERROR_SUCCESS as i32;
    let open_status = unsafe {
        RegOpenKeyExW(
            root,
            subkey_wide.as_ptr(),
            0,
            KEY_READ | KEY_WOW64_64KEY,
            &mut key,
        )
    };
    if open_status != success || key.is_null() {
        return None;
    }

    let mut value_type: DWORD = 0;
    let mut byte_len: DWORD = 0;
    let query_status = unsafe {
        RegQueryValueExW(
            key,
            value_wide.as_ptr(),
            ptr::null_mut(),
            &mut value_type,
            ptr::null_mut(),
            &mut byte_len,
        )
    };
    if query_status != success || byte_len == 0 {
        unsafe {
            RegCloseKey(key);
        }
        return None;
    }

    let mut bytes = vec![0u8; byte_len as usize];
    let query_status = unsafe {
        RegQueryValueExW(
            key,
            value_wide.as_ptr(),
            ptr::null_mut(),
            &mut value_type,
            bytes.as_mut_ptr(),
            &mut byte_len,
        )
    };
    unsafe {
        RegCloseKey(key);
    }
    if query_status != success {
        return None;
    }
    bytes.truncate(byte_len as usize);
    Some((value_type, bytes))
}

#[cfg(target_os = "windows")]
fn registry_utf16_string(bytes: &[u8]) -> String {
    let words: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
        .collect();
    String::from_utf16_lossy(&words)
}
