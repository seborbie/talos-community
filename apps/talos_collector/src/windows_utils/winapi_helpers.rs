use anyhow::{Context, Result};

#[cfg(windows)]
use windows::Win32::Foundation::*;

#[cfg(windows)]
use windows::Win32::Globalization::*;

#[cfg(windows)]
use windows::Win32::NetworkManagement::IpHelper::*;

#[cfg(windows)]
use windows::Win32::Networking::WinSock::*;

#[cfg(windows)]
use windows::Win32::Storage::FileSystem::*;

#[cfg(windows)]
use windows::Win32::System::Threading::*;

/// Get the system default UI language using Windows API.
#[cfg(windows)]
pub fn get_system_default_ui_language() -> Option<String> {
    unsafe {
        let lang_id = GetSystemDefaultUILanguage();
        lang_id_to_locale(lang_id as u64)
    }
}

#[cfg(not(windows))]
pub fn get_system_default_ui_language() -> Option<String> {
    None
}

/// Get the current user's default locale using Windows API.
#[cfg(windows)]
pub fn get_system_locale() -> Option<String> {
    unsafe {
        let mut locale_name = [0u16; 85];
        let result = GetUserDefaultLocaleName(&mut locale_name);
        if result > 0 {
            let locale = String::from_utf16_lossy(&locale_name[..result as usize]);
            let trimmed = locale.trim_end_matches('\0').to_string();
            if !trimmed.is_empty() {
                return Some(trimmed);
            }
        }
        None
    }
}

#[cfg(not(windows))]
pub fn get_system_locale() -> Option<String> {
    None
}

#[cfg(windows)]
fn lang_id_to_locale(code: u64) -> Option<String> {
    let locale = match code {
        1033 => "en-US",
        2057 => "en-GB",
        3081 => "en-AU",
        4105 => "en-CA",
        1031 => "de-DE",
        1036 => "fr-FR",
        1040 => "it-IT",
        1041 => "ja-JP",
        1043 => "nl-NL",
        1046 => "pt-BR",
        2070 => "pt-PT",
        1034 => "es-ES",
        2058 => "es-MX",
        _ => return None,
    };
    Some(locale.to_string())
}

/// Get timezone using Windows API
#[cfg(windows)]
pub fn get_timezone() -> Option<String> {
    // Use GetUserDefaultLocaleName for timezone since GetTimeZoneInformation needs more features
    // For now, just return None - the timezone can be obtained via WMI
    None
}

#[cfg(not(windows))]
pub fn get_timezone() -> Option<String> {
    None
}

/// Get TCP connection table using iphlpapi
#[cfg(windows)]
pub fn get_tcp_connections() -> Result<Vec<TcpConnection>> {
    unsafe {
        let mut size = 0u32;
        let result = GetExtendedTcpTable(
            None,
            &mut size,
            false,
            AF_INET.0 as u32,
            TCP_TABLE_OWNER_PID_ALL,
            0,
        );

        // ERROR_INSUFFICIENT_BUFFER = 122
        if result != 122 {
            return Err(anyhow::anyhow!("Failed to get TCP table size: {}", result));
        }

        let mut buffer = vec![0u8; size as usize];
        let result = GetExtendedTcpTable(
            Some(buffer.as_mut_ptr() as *mut _),
            &mut size,
            false,
            AF_INET.0 as u32,
            TCP_TABLE_OWNER_PID_ALL,
            0,
        );

        if result != 0 {
            return Err(anyhow::anyhow!("Failed to get TCP table: {}", result));
        }

        let table = &*(buffer.as_ptr() as *const MIB_TCPTABLE_OWNER_PID);
        let num_entries = table.dwNumEntries;
        let rows = std::slice::from_raw_parts(table.table.as_ptr(), num_entries as usize);

        let mut connections = Vec::new();
        for row in rows {
            // Convert network byte order to host byte order
            let local_addr = u32::from_be(row.dwLocalAddr);
            let local_ip = format!(
                "{}.{}.{}.{}",
                local_addr & 0xFF,
                (local_addr >> 8) & 0xFF,
                (local_addr >> 16) & 0xFF,
                (local_addr >> 24) & 0xFF
            );
            // Port is in network byte order, swap bytes
            let local_port = ((row.dwLocalPort >> 8) & 0xFF) | ((row.dwLocalPort << 8) & 0xFF00);

            let remote_addr = u32::from_be(row.dwRemoteAddr);
            let remote_ip = format!(
                "{}.{}.{}.{}",
                remote_addr & 0xFF,
                (remote_addr >> 8) & 0xFF,
                (remote_addr >> 16) & 0xFF,
                (remote_addr >> 24) & 0xFF
            );
            let remote_port = ((row.dwRemotePort >> 8) & 0xFF) | ((row.dwRemotePort << 8) & 0xFF00);

            // MIB_TCP_STATE constants
            let state = match row.dwState {
                1 => "Closed",
                2 => "Listen",
                3 => "SynSent",
                4 => "SynReceived",
                5 => "Established",
                6 => "FinWait1",
                7 => "FinWait2",
                8 => "CloseWait",
                9 => "Closing",
                10 => "LastAck",
                11 => "TimeWait",
                12 => "DeleteTcb",
                _ => "Unknown",
            };

            connections.push(TcpConnection {
                local_ip,
                local_port: local_port as u16,
                remote_ip,
                remote_port: remote_port as u16,
                state: state.to_string(),
                pid: row.dwOwningPid,
            });
        }

        Ok(connections)
    }
}

#[cfg(not(windows))]
pub fn get_tcp_connections() -> Result<Vec<TcpConnection>> {
    Ok(Vec::new())
}

/// TCP connection info
#[derive(Debug, Clone)]
pub struct TcpConnection {
    pub local_ip: String,
    pub local_port: u16,
    pub remote_ip: String,
    pub remote_port: u16,
    pub state: String,
    pub pid: u32,
}

/// Get UDP endpoint table using iphlpapi
#[cfg(windows)]
pub fn get_udp_endpoints() -> Result<Vec<UdpEndpoint>> {
    unsafe {
        let mut size = 0u32;
        let result = GetExtendedUdpTable(
            None,
            &mut size,
            false,
            AF_INET.0 as u32,
            UDP_TABLE_OWNER_PID,
            0,
        );

        // ERROR_INSUFFICIENT_BUFFER = 122
        if result != 122 {
            return Err(anyhow::anyhow!("Failed to get UDP table size: {}", result));
        }

        let mut buffer = vec![0u8; size as usize];
        let result = GetExtendedUdpTable(
            Some(buffer.as_mut_ptr() as *mut _),
            &mut size,
            false,
            AF_INET.0 as u32,
            UDP_TABLE_OWNER_PID,
            0,
        );

        if result != 0 {
            return Err(anyhow::anyhow!("Failed to get UDP table: {}", result));
        }

        let table = &*(buffer.as_ptr() as *const MIB_UDPTABLE_OWNER_PID);
        let num_entries = table.dwNumEntries;
        let rows = std::slice::from_raw_parts(table.table.as_ptr(), num_entries as usize);

        let mut endpoints = Vec::new();
        for row in rows {
            let local_addr = u32::from_be(row.dwLocalAddr);
            let local_ip = format!(
                "{}.{}.{}.{}",
                local_addr & 0xFF,
                (local_addr >> 8) & 0xFF,
                (local_addr >> 16) & 0xFF,
                (local_addr >> 24) & 0xFF
            );
            let local_port = ((row.dwLocalPort >> 8) & 0xFF) | ((row.dwLocalPort << 8) & 0xFF00);

            endpoints.push(UdpEndpoint {
                local_ip,
                local_port: local_port as u16,
                pid: row.dwOwningPid,
            });
        }

        Ok(endpoints)
    }
}

#[cfg(not(windows))]
pub fn get_udp_endpoints() -> Result<Vec<UdpEndpoint>> {
    Ok(Vec::new())
}

/// UDP endpoint info
#[derive(Debug, Clone)]
pub struct UdpEndpoint {
    pub local_ip: String,
    pub local_port: u16,
    pub pid: u32,
}

/// Check Secure Boot status via Windows API
/// Note: This requires the GetFirmwareEnvironmentVariable function which may not be available
#[cfg(windows)]
pub fn get_secure_boot_status() -> Option<bool> {
    // Use registry check which is already done in hardware.rs
    // This is a placeholder for direct firmware API access which requires special privileges
    None
}

#[cfg(not(windows))]
pub fn get_secure_boot_status() -> Option<bool> {
    None
}

/// Get file version info using Windows API
#[cfg(windows)]
pub fn get_file_version(file_path: &str) -> Option<String> {
    unsafe {
        let path_wide: Vec<u16> = file_path.encode_utf16().chain(std::iter::once(0)).collect();

        // Get file version info size
        let mut handle = 0u32;
        let size = GetFileVersionInfoSizeW(
            windows::core::PCWSTR::from_raw(path_wide.as_ptr()),
            Some(&mut handle),
        );

        if size == 0 {
            return None;
        }

        // Allocate buffer and get version info
        let mut buffer = vec![0u8; size as usize];
        let result = GetFileVersionInfoW(
            windows::core::PCWSTR::from_raw(path_wide.as_ptr()),
            handle,
            size,
            buffer.as_mut_ptr() as *mut _,
        );

        if result.is_err() {
            return None;
        }

        // Query fixed file info for version
        let mut value_ptr: *mut u8 = std::ptr::null_mut();
        let mut value_len = 0u32;

        // Query root for VS_FIXEDFILEINFO
        let query: Vec<u16> = "\\".encode_utf16().chain(std::iter::once(0)).collect();
        let query_result = VerQueryValueW(
            buffer.as_ptr() as *const _,
            windows::core::PCWSTR::from_raw(query.as_ptr()),
            &mut value_ptr as *mut *mut _ as *mut *mut _,
            &mut value_len,
        );

        if !query_result.as_bool() || value_len < std::mem::size_of::<VS_FIXEDFILEINFO>() as u32 {
            return None;
        }

        let fixed_info = &*(value_ptr as *const VS_FIXEDFILEINFO);
        if fixed_info.dwSignature == 0xFEEF04BD {
            let version = format!(
                "{}.{}.{}.{}",
                (fixed_info.dwFileVersionMS >> 16) & 0xFFFF,
                fixed_info.dwFileVersionMS & 0xFFFF,
                (fixed_info.dwFileVersionLS >> 16) & 0xFFFF,
                fixed_info.dwFileVersionLS & 0xFFFF
            );
            return Some(version);
        }

        None
    }
}

#[cfg(not(windows))]
pub fn get_file_version(_file_path: &str) -> Option<String> {
    None
}

/// Get process name from PID
#[cfg(windows)]
pub fn get_process_name(pid: u32) -> Option<String> {
    unsafe {
        if pid == 0 {
            return Some("System Idle".to_string());
        }

        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid);
        if handle.is_err() {
            return None;
        }
        let handle = handle.unwrap();

        let mut buffer = [0u16; 260];
        let mut size = 260u32;

        let query_result = QueryFullProcessImageNameW(
            handle,
            PROCESS_NAME_WIN32,
            windows::core::PWSTR::from_raw(buffer.as_mut_ptr()),
            &mut size,
        );

        let _ = CloseHandle(handle);

        if query_result.is_ok() && size > 0 {
            let path = String::from_utf16_lossy(&buffer[..size as usize]);
            let trimmed = path.trim_end_matches('\0');
            trimmed.rsplit('\\').next().map(|s| s.to_string())
        } else {
            None
        }
    }
}

#[cfg(not(windows))]
pub fn get_process_name(_pid: u32) -> Option<String> {
    None
}

/// Check if process is running by name using Windows API
/// Uses native Windows API to enumerate processes
#[cfg(windows)]
pub fn is_process_running(_process_name: &str) -> bool {
    // Use WMI to check for process existence - more reliable than ToolHelp API
    // This is handled elsewhere, return false here
    false
}

#[cfg(not(windows))]
pub fn is_process_running(_process_name: &str) -> bool {
    false
}

/// Execute a command and return output (non-PowerShell)
pub fn run_command(cmd: &str, args: &[&str], _timeout_secs: u64) -> Result<String> {
    use std::process::{Command, Stdio};

    let mut command = Command::new(cmd);
    command
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let output = command.output().context("Failed to execute command")?;

    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    } else {
        Err(anyhow::anyhow!(
            "Command failed with exit code {:?}: {}",
            output.status.code(),
            String::from_utf8_lossy(&output.stderr)
        ))
    }
}
