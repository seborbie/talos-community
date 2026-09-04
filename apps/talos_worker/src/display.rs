//! Virtual display check, initialization, and parameter control (Windows only).
//!
//! The agent calls [`ensure_display_ready`] before starting the capture pipeline.
//! This check is best-effort and does not gate tunnel bring-up.
//! If DXGI works straight away (physical monitor attached) we proceed immediately.
//! If DXGI fails (headless / no monitor) we refresh the display topology via
//! `SetDisplayConfig(SDC_TOPOLOGY_EXTEND | SDC_APPLY)` so a virtual
//! monitor becomes active, wait briefly, then retry DXGI.

#![cfg(windows)]

use std::path::Path;
use std::ptr;

use tracing::{debug, warn};

#[derive(Debug, Clone, Copy)]
pub struct LaunchedCaptureHelper {
    pub process_handle: usize,
    pub process_id: u32,
}

// COM must be initialized on the thread before D3D11/DXGI calls (e.g. DxgiBackend::new).
// spawn_blocking worker threads often have no COM state, which can cause
// D3D11CreateDevice to fail with E_INVALIDARG (0x80070057).
fn ensure_com_initialized() {
    use winapi::um::combaseapi::CoInitializeEx;
    use winapi::um::objbase::COINIT_MULTITHREADED;

    unsafe {
        let hr = CoInitializeEx(ptr::null_mut(), COINIT_MULTITHREADED);
        // S_OK (0) = we initialized; S_FALSE (1) = already initialized on this thread.
        if hr == 0 || hr == 1 {
            // OK
        } else {
            tracing::warn!(
                hr = hr,
                "CoInitializeEx failed (DXGI check may still work if COM was set elsewhere)"
            );
        }
    }
}

fn is_non_console_capture_target(
    target_session_id: u32,
    active_console_session_id: u32,
    win_station_name: Option<&str>,
) -> bool {
    const INVALID_SESSION: u32 = 0xFFFF_FFFF;

    if active_console_session_id != INVALID_SESSION
        && target_session_id == active_console_session_id
    {
        return false;
    }

    if win_station_name
        .map(|name| name.eq_ignore_ascii_case("Console"))
        .unwrap_or(false)
    {
        return false;
    }

    true
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CaptureLaunchPlan {
    desktop_target: &'static str,
    prefer_system_token: bool,
    allow_winlogon_token: bool,
}

fn resolve_capture_launch_plan(
    is_non_console_target: bool,
    probed_input_desktop_name: Option<&str>,
) -> CaptureLaunchPlan {
    let secure_input_desktop = probed_input_desktop_name
        .map(crate::control::classify_desktop_name)
        .map(|context| {
            !matches!(
                context,
                crate::control::DesktopContext::Default | crate::control::DesktopContext::Unknown
            )
        })
        .unwrap_or(false);
    let allow_winlogon_token = !is_non_console_target
        && probed_input_desktop_name
            .map(|name| name.eq_ignore_ascii_case("Winlogon"))
            .unwrap_or(false);

    CaptureLaunchPlan {
        // Keep the helper on the interactive default desktop and let the
        // capture thread attach to the active input desktop itself. Launching
        // directly onto Winlogon is brittle on some NVIDIA systems and can
        // leave DXGI timing out before any real frame is produced.
        desktop_target: "winsta0\\default",
        prefer_system_token: !is_non_console_target && secure_input_desktop,
        allow_winlogon_token,
    }
}

// ---------------------------------------------------------------------------
// SetDisplayConfig constants and FFI
// ---------------------------------------------------------------------------

const SDC_TOPOLOGY_EXTEND: u32 = 0x0000_0004; // 0x10 is SDC_TOPOLOGY_SUPPLIED (requires path array); 0x04 = extend from DB
const SDC_APPLY: u32 = 0x0000_0080;

extern "system" {
    fn SetDisplayConfig(
        num_path_array_elements: u32,
        path_array: *const u8,
        num_mode_info_array_elements: u32,
        mode_info_array: *const u8,
        flags: u32,
    ) -> i32;
}

// ---------------------------------------------------------------------------
// Console-session helper (for service / non-console session)
// ---------------------------------------------------------------------------

/// Called when the process is launched with `--display-init-helper`. Runs in the console
/// session and calls SetDisplayConfig so display topology is refreshed. Exits immediately.
/// Used when the main agent runs as a service (Session 0) or in RDP (Session 2).
pub fn run_display_init_helper() -> Result<(), ()> {
    ensure_com_initialized();
    let flags = SDC_TOPOLOGY_EXTEND | SDC_APPLY;
    let result = unsafe { SetDisplayConfig(0, ptr::null(), 0, ptr::null(), flags) };
    if result == 0 {
        Ok(())
    } else {
        Err(())
    }
}

// ---------------------------------------------------------------------------
// Headless auto-logon (removes "user must be logged in" requirement)
// ---------------------------------------------------------------------------

const WINLOGON_PATH: &str = "SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion\\Winlogon";

/// Configure Windows to auto-logon the given user at boot so a console session
/// exists for display initialization when the agent runs as a service. Requires admin.
/// Password is stored in registry (plaintext); use a dedicated limited account.
pub fn configure_headless_auto_logon(
    username: &str,
    password: &str,
    domain: Option<&str>,
) -> Result<(), String> {
    if username.is_empty() || password.is_empty() {
        return Err("username and password required".to_string());
    }
    let domain = domain.unwrap_or(".");
    unsafe {
        use std::os::windows::ffi::OsStrExt;
        use winapi::shared::minwindef::HKEY;
        use winapi::um::winnt::{KEY_SET_VALUE, KEY_WOW64_64KEY, REG_SZ};
        use winapi::um::winreg::{RegCloseKey, RegOpenKeyExW, RegSetValueExW, HKEY_LOCAL_MACHINE};

        let path_wide: Vec<u16> = std::ffi::OsStr::new(WINLOGON_PATH)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let mut hkey: HKEY = std::ptr::null_mut();
        let err = RegOpenKeyExW(
            HKEY_LOCAL_MACHINE,
            path_wide.as_ptr(),
            0,
            KEY_SET_VALUE | KEY_WOW64_64KEY,
            &mut hkey,
        );
        if err != 0 {
            return Err(format!("RegOpenKeyExW Winlogon failed: {}", err));
        }

        fn set_reg_sz(hkey: HKEY, name: &str, value: &str) -> Result<(), String> {
            let name_wide: Vec<u16> = std::ffi::OsStr::new(name)
                .encode_wide()
                .chain(std::iter::once(0))
                .collect();
            let value_wide: Vec<u16> = value.encode_utf16().chain(std::iter::once(0)).collect();
            let size = (value_wide.len() * 2) as u32;
            let err = unsafe {
                RegSetValueExW(
                    hkey,
                    name_wide.as_ptr(),
                    0,
                    REG_SZ,
                    value_wide.as_ptr() as *const _,
                    size,
                )
            };
            if err != 0 {
                return Err(format!("RegSetValueExW {} failed: {}", name, err));
            }
            Ok(())
        }
        set_reg_sz(hkey, "DefaultUserName", username)?;
        set_reg_sz(hkey, "DefaultPassword", password)?;
        set_reg_sz(hkey, "DefaultDomainName", domain)?;
        set_reg_sz(hkey, "AutoAdminLogon", "1")?;
        let _ = RegCloseKey(hkey);
        Ok(())
    }
}

/// Log WTS session list and state of target session for diagnostics (e.g. why WTSQueryUserToken fails).
#[derive(Debug, Clone)]
pub struct EnumeratedWtsSession {
    pub session_id: u32,
    pub logical_session_id: u32,
    pub native_session_id: u32,
    pub kind: String,
    pub win_station: String,
    pub user_name: String,
    pub state: String,
}

/// Enumerate interactive Windows sessions for session-switch UI.
///
/// Returns session id, window-station name, resolved username (`DOMAIN\user` when available),
/// and normalized state string (`active`, `disconnected`, `connected`, `idle`, `unknown`).
pub fn enumerate_wts_sessions() -> Vec<EnumeratedWtsSession> {
    use std::ffi::OsString;
    use std::os::windows::ffi::OsStringExt;
    use winapi::shared::minwindef::{BOOL, DWORD, LPVOID};
    use winapi::shared::ntdef::HANDLE;

    #[repr(C)]
    #[allow(non_snake_case)]
    struct WTS_SESSION_INFO {
        SessionId: DWORD,
        pWinStationName: *mut u16,
        State: DWORD,
    }

    #[repr(u32)]
    enum WtsInfoClass {
        WTSUserName = 5,
        WTSDomainName = 7,
    }

    extern "system" {
        fn WTSEnumerateSessionsW(
            hServer: HANDLE,
            Reserved: DWORD,
            Version: DWORD,
            ppSessionInfo: *mut *mut WTS_SESSION_INFO,
            pCount: *mut DWORD,
        ) -> BOOL;
        fn WTSQuerySessionInformationW(
            hServer: HANDLE,
            SessionId: DWORD,
            WTSInfoClass: u32,
            ppBuffer: *mut LPVOID,
            pBytesReturned: *mut DWORD,
        ) -> BOOL;
        fn WTSFreeMemory(pMemory: LPVOID);
    }

    let query_session_string = |session_id: DWORD, info_class: WtsInfoClass| -> Option<String> {
        unsafe {
            let mut buffer: LPVOID = ptr::null_mut();
            let mut bytes: DWORD = 0;
            let ok = WTSQuerySessionInformationW(
                ptr::null_mut(),
                session_id,
                info_class as u32,
                &mut buffer as *mut _,
                &mut bytes as *mut _,
            );
            if ok == 0 || buffer.is_null() || bytes <= 1 {
                if !buffer.is_null() {
                    WTSFreeMemory(buffer);
                }
                return None;
            }
            let len = bytes as usize / 2;
            let slice = std::slice::from_raw_parts(buffer as *const u16, len);
            let nul = slice.iter().position(|c| *c == 0).unwrap_or(len);
            let value = OsString::from_wide(&slice[..nul])
                .to_string_lossy()
                .trim()
                .to_string();
            WTSFreeMemory(buffer);
            if value.is_empty() {
                None
            } else {
                Some(value)
            }
        }
    };

    let active_console_session_id = unsafe { winapi::um::winbase::WTSGetActiveConsoleSessionId() };
    let mut out = Vec::new();
    unsafe {
        let mut sessions_ptr: *mut WTS_SESSION_INFO = ptr::null_mut();
        let mut count: DWORD = 0;
        let ok = WTSEnumerateSessionsW(ptr::null_mut(), 0, 1, &mut sessions_ptr, &mut count);
        if ok == 0 || sessions_ptr.is_null() {
            debug!("enumerate_wts_sessions: WTSEnumerateSessionsW failed");
            return out;
        }

        let sessions = std::slice::from_raw_parts(sessions_ptr, count as usize);
        for s in sessions {
            let win_station = if s.pWinStationName.is_null() {
                String::new()
            } else {
                let len = (0..)
                    .take_while(|&i| *s.pWinStationName.add(i) != 0)
                    .count();
                OsString::from_wide(std::slice::from_raw_parts(s.pWinStationName, len))
                    .to_string_lossy()
                    .into_owned()
            };

            let user =
                query_session_string(s.SessionId, WtsInfoClass::WTSUserName).unwrap_or_default();
            let domain =
                query_session_string(s.SessionId, WtsInfoClass::WTSDomainName).unwrap_or_default();
            let user_name = if !domain.is_empty() && !user.is_empty() {
                format!("{domain}\\{user}")
            } else if !user.is_empty() {
                user
            } else {
                String::new()
            };

            let state = match s.State {
                0 => "active",
                1 => "connected",
                4 => "disconnected",
                5 => "idle",
                6 => "listen",
                _ => "unknown",
            }
            .to_string();

            let is_console = s.SessionId == active_console_session_id
                || win_station.eq_ignore_ascii_case("Console");
            let is_user_session = !user_name.trim().is_empty()
                && matches!(
                    state.as_str(),
                    "active" | "connected" | "disconnected" | "idle"
                );
            if !is_console && !is_user_session {
                continue;
            }
            if s.SessionId == 0 || s.SessionId >= 65_536 {
                continue;
            }

            out.push(EnumeratedWtsSession {
                session_id: s.SessionId,
                logical_session_id: 0,
                native_session_id: s.SessionId,
                kind: if is_console { "console" } else { "rdp" }.to_string(),
                win_station,
                user_name,
                state,
            });
        }
        WTSFreeMemory(sessions_ptr as *mut _);
    }

    if active_console_session_id != 0xFFFF_FFFF
        && active_console_session_id > 0
        && active_console_session_id < 65_536
        && !out
            .iter()
            .any(|session| session.session_id == active_console_session_id)
    {
        out.push(EnumeratedWtsSession {
            session_id: active_console_session_id,
            logical_session_id: 0,
            native_session_id: active_console_session_id,
            kind: "console".to_string(),
            win_station: "Console".to_string(),
            user_name: String::new(),
            state: "unknown".to_string(),
        });
    }

    out.sort_by_key(|s| (if s.kind == "console" { 0 } else { 1 }, s.session_id));
    let mut next_rdp_logical_id = 2;
    for session in &mut out {
        if session.kind == "console" {
            session.logical_session_id = 1;
        } else {
            session.logical_session_id = next_rdp_logical_id;
            next_rdp_logical_id += 1;
        }
    }
    out
}

/// Request logoff for a specific WTS session.
pub fn logoff_wts_session(session_id: u32) -> bool {
    use winapi::shared::minwindef::DWORD;
    use winapi::shared::ntdef::HANDLE;
    use winapi::um::errhandlingapi::GetLastError;

    extern "system" {
        fn WTSLogoffSession(hServer: HANDLE, SessionId: DWORD, bWait: i32) -> i32;
    }

    if session_id == 0 || session_id >= 65_536 {
        debug!(
            session_id = session_id,
            "logoff_wts_session: invalid session id"
        );
        return false;
    }

    unsafe {
        let ok = WTSLogoffSession(ptr::null_mut(), session_id as DWORD, 0);
        if ok == 0 {
            let err = GetLastError();
            warn!(
                session_id = session_id,
                error = err,
                "logoff_wts_session: WTSLogoffSession failed"
            );
            false
        } else {
            debug!(session_id = session_id, "logoff_wts_session: requested");
            true
        }
    }
}

fn log_wts_sessions_diagnostics(target_session_id: u32) {
    use std::ffi::OsString;
    use std::os::windows::ffi::OsStringExt;
    use winapi::shared::minwindef::{BOOL, DWORD, LPVOID};
    use winapi::shared::ntdef::HANDLE;

    #[repr(C)]
    #[allow(non_snake_case)]
    struct WTS_SESSION_INFO {
        SessionId: DWORD,
        pWinStationName: *mut u16,
        State: DWORD,
    }

    extern "system" {
        fn WTSEnumerateSessionsW(
            hServer: HANDLE,
            Reserved: DWORD,
            Version: DWORD,
            ppSessionInfo: *mut *mut WTS_SESSION_INFO,
            pCount: *mut DWORD,
        ) -> BOOL;
        fn WTSFreeMemory(pMemory: LPVOID);
    }

    const WTS_STATE_NAMES: &[&str] = &[
        "Active",
        "Connected",
        "ConnectQuery",
        "Shadow",
        "Disconnected",
        "Idle",
        "Listen",
        "Reset",
        "Down",
        "Init",
    ];

    unsafe {
        let mut sessions_ptr: *mut WTS_SESSION_INFO = ptr::null_mut();
        let mut count: DWORD = 0;
        let ok = WTSEnumerateSessionsW(ptr::null_mut(), 0, 1, &mut sessions_ptr, &mut count);
        if ok == 0 || sessions_ptr.is_null() {
            debug!("launch_capture_helper: WTSEnumerateSessionsW failed");
            return;
        }
        let sessions = std::slice::from_raw_parts(sessions_ptr, count as usize);
        for s in sessions {
            let state_name = (s.State as usize)
                .min(WTS_STATE_NAMES.len().saturating_sub(1))
                .min(10);
            let name = if s.pWinStationName.is_null() {
                String::from("(null)")
            } else {
                let len = (0..)
                    .take_while(|&i| *s.pWinStationName.add(i) != 0)
                    .count();
                OsString::from_wide(std::slice::from_raw_parts(s.pWinStationName, len))
                    .to_string_lossy()
                    .into_owned()
            };
            let is_target = s.SessionId == target_session_id;
            debug!(
                session_id = s.SessionId,
                state = s.State,
                state_name = %WTS_STATE_NAMES[state_name],
                win_station = %name,
                is_target = is_target,
                "launch_capture_helper: WTS session"
            );
        }
        WTSFreeMemory(sessions_ptr as *mut _);
    }
}

/// Generate a software secure-attention sequence from the agent service process.
/// If a target session user token is available, impersonate it so SAS is directed
/// at that session; otherwise fall back to the active console/default service path.
pub fn send_secure_attention_for_session(target_session_id: Option<u32>) -> Result<(), String> {
    #[link(name = "sas")]
    extern "system" {
        fn SendSAS(as_user: i32);
    }

    unsafe {
        use winapi::shared::minwindef::FALSE;
        use winapi::shared::ntdef::HANDLE;
        use winapi::um::errhandlingapi::GetLastError;
        use winapi::um::handleapi::CloseHandle;
        use winapi::um::securitybaseapi::{ImpersonateLoggedOnUser, RevertToSelf};
        use winapi::um::wtsapi32::WTSQueryUserToken;

        let resolved_session_id = target_session_id
            .filter(|sid| *sid > 0 && *sid < 65_536)
            .unwrap_or_else(|| winapi::um::winbase::WTSGetActiveConsoleSessionId());

        let mut token: HANDLE = ptr::null_mut();
        let mut impersonated = false;

        if resolved_session_id > 0
            && resolved_session_id != 0xFFFF_FFFF
            && WTSQueryUserToken(resolved_session_id, &mut token) != 0
            && !token.is_null()
        {
            if ImpersonateLoggedOnUser(token) != 0 {
                impersonated = true;
            } else {
                let err = GetLastError();
                debug!(
                    target_session_id = resolved_session_id,
                    error = err,
                    "send_secure_attention: impersonation failed; falling back to service context"
                );
            }
        }

        debug!(
            target_session_id = resolved_session_id,
            impersonated = impersonated,
            "send_secure_attention: dispatching SendSAS from agent service"
        );
        SendSAS(if impersonated { 1 } else { FALSE });

        if impersonated {
            let _ = RevertToSelf();
        }
        if !token.is_null() {
            let _ = CloseHandle(token);
        }
        Ok(())
    }
}

/// Launch `talos_worker_helper.exe` in the console session for capture.
/// Requires running as LocalSystem (e.g. Windows service) for WTSQueryUserToken.
pub fn launch_capture_helper_in_console_session(
    helper_path: &Path,
    rmm_session_id: &str,
    session_seq: u64,
    pipe_instance: u64,
    pipe_name: &str,
    control_pipe_name: &str,
    auth_token: &str,
    display_stream_mode: &str,
    display_processing_mode: &str,
    console_session_id: u32,
) -> Option<LaunchedCaptureHelper> {
    debug!(
        helper_path = %helper_path.display(),
        rmm_session_id = %rmm_session_id,
        session_seq = session_seq,
        pipe_instance = pipe_instance,
        pipe_name = %pipe_name,
        console_session_id = console_session_id,
        "launch_capture_helper: entry"
    );
    log_wts_sessions_diagnostics(console_session_id);
    const INVALID_SESSION: u32 = 0xFFFFFFFF;
    if console_session_id == INVALID_SESSION || console_session_id == 0 {
        debug!(
            console_session_id = console_session_id,
            "launch_capture_helper: invalid or zero console session id"
        );
        return None;
    }
    // Launch a SINGLE helper that runs both:
    // - control pipe loop (input/control messages including stop_capture)
    // - capture/encode loop (DXGI capture)
    //
    // This avoids a regression where both a dedicated input-only helper and the capture helper
    // would race to connect to the same control pipe. The control pipe is created with
    // max instances = 1, so one of them would time out and capture would not stop promptly.
    let display_processing_for_child = display_processing_mode.trim();
    debug!(
        display_stream_mode = display_stream_mode,
        display_processing_mode = display_processing_for_child,
        "launch_capture_helper: resolved helper display modes"
    );
    let cmd_line_capture = format!(
        "\"{}\" --pipe \"{}\" --control-pipe \"{}\" --auth \"{}\" --rmm-session-id \"{}\" --session-seq {} --pipe-instance {} --display-stream-mode \"{}\" --display-processing-mode \"{}\"",
        helper_path.display(),
        pipe_name,
        control_pipe_name,
        auth_token,
        rmm_session_id,
        session_seq,
        pipe_instance,
        display_stream_mode,
        display_processing_for_child
    );
    let cmd_capture_wide: Vec<u16> = cmd_line_capture
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();

    unsafe {
        use winapi::shared::minwindef::DWORD;
        use winapi::shared::minwindef::FALSE;
        use winapi::shared::minwindef::LPVOID;
        use winapi::shared::ntdef::HANDLE;
        use winapi::um::errhandlingapi::GetLastError;
        use winapi::um::handleapi::CloseHandle;
        use winapi::um::handleapi::INVALID_HANDLE_VALUE;
        use winapi::um::processthreadsapi::GetCurrentProcess;
        use winapi::um::processthreadsapi::GetCurrentProcessId;
        use winapi::um::processthreadsapi::OpenProcess;
        use winapi::um::processthreadsapi::OpenProcessToken;
        use winapi::um::processthreadsapi::ProcessIdToSessionId;
        use winapi::um::securitybaseapi::{AdjustTokenPrivileges, SetTokenInformation};
        use winapi::um::securitybaseapi::{DuplicateTokenEx, GetTokenInformation};
        use winapi::um::tlhelp32::{
            CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
            TH32CS_SNAPPROCESS,
        };
        use winapi::um::winbase::LookupPrivilegeValueW;
        use winapi::um::winbase::{ABOVE_NORMAL_PRIORITY_CLASS, CREATE_NO_WINDOW};
        use winapi::um::winnt::PROCESS_QUERY_LIMITED_INFORMATION;
        use winapi::um::winnt::{
            SecurityImpersonation, TokenElevation, TokenElevationType, TokenLinkedToken,
            TokenPrimary, TokenSessionId, TokenStatistics, TokenType, MAXIMUM_ALLOWED,
            TOKEN_ADJUST_DEFAULT, TOKEN_ADJUST_SESSIONID, TOKEN_ASSIGN_PRIMARY, TOKEN_DUPLICATE,
            TOKEN_ELEVATION, TOKEN_ELEVATION_TYPE, TOKEN_LINKED_TOKEN, TOKEN_PRIVILEGES,
            TOKEN_QUERY, TOKEN_STATISTICS,
        };
        use winapi::um::wtsapi32::WTSQueryUserToken;

        #[repr(u32)]
        enum WtsInfoClass {
            WTSUserName = 5,
            WTSWinStationName = 6,
            WTSDomainName = 7,
            WTSConnectState = 8,
        }

        extern "system" {
            fn WTSQuerySessionInformationW(
                hServer: HANDLE,
                SessionId: DWORD,
                WTSInfoClass: u32,
                ppBuffer: *mut LPVOID,
                pBytesReturned: *mut DWORD,
            ) -> i32;
            fn WTSFreeMemory(pMemory: LPVOID);
        }

        let query_wts_string = |session_id: u32, info_class: u32| -> Option<String> {
            let mut buffer: LPVOID = ptr::null_mut();
            let mut bytes: DWORD = 0;
            let ok = WTSQuerySessionInformationW(
                ptr::null_mut() as HANDLE,
                session_id,
                info_class,
                &mut buffer as *mut _,
                &mut bytes as *mut _,
            );
            if ok == 0 || buffer.is_null() || bytes <= 1 {
                if ok == 0 {
                    let err = GetLastError();
                    warn!(
                        target_session_id = session_id,
                        info_class = info_class,
                        error = err,
                        "launch_capture_helper: WTSQuerySessionInformationW (string) failed"
                    );
                }
                if !buffer.is_null() {
                    WTSFreeMemory(buffer);
                }
                return None;
            }
            let len = bytes as usize / 2;
            let slice = std::slice::from_raw_parts(buffer as *const u16, len);
            let nul = slice.iter().position(|c| *c == 0).unwrap_or(len);
            let value = String::from_utf16_lossy(&slice[..nul]);
            WTSFreeMemory(buffer);
            Some(value)
        };

        let query_wts_u32 = |session_id: u32, info_class: u32| -> Option<u32> {
            let mut buffer: LPVOID = ptr::null_mut();
            let mut bytes: DWORD = 0;
            let ok = WTSQuerySessionInformationW(
                ptr::null_mut() as HANDLE,
                session_id,
                info_class,
                &mut buffer as *mut _,
                &mut bytes as *mut _,
            );
            if ok == 0 || buffer.is_null() || bytes < std::mem::size_of::<u32>() as u32 {
                if ok == 0 {
                    let err = GetLastError();
                    warn!(
                        target_session_id = session_id,
                        info_class = info_class,
                        error = err,
                        "launch_capture_helper: WTSQuerySessionInformationW (u32) failed"
                    );
                }
                if !buffer.is_null() {
                    WTSFreeMemory(buffer);
                }
                return None;
            }
            let value = *(buffer as *const u32);
            WTSFreeMemory(buffer);
            Some(value)
        };

        let target_wts_user =
            query_wts_string(console_session_id, WtsInfoClass::WTSUserName as u32)
                .unwrap_or_else(|| "<none>".to_string());
        let target_wts_domain =
            query_wts_string(console_session_id, WtsInfoClass::WTSDomainName as u32)
                .unwrap_or_else(|| "<none>".to_string());
        let target_wts_state =
            query_wts_u32(console_session_id, WtsInfoClass::WTSConnectState as u32)
                .unwrap_or(u32::MAX);
        const WTS_STATE_NAMES_LOCAL: &[&str] = &[
            "Active",
            "Connected",
            "ConnectQuery",
            "Shadow",
            "Disconnected",
            "Idle",
            "Listen",
            "Reset",
            "Down",
            "Init",
        ];
        let target_wts_state_name = if (target_wts_state as usize) < WTS_STATE_NAMES_LOCAL.len() {
            WTS_STATE_NAMES_LOCAL[target_wts_state as usize]
        } else {
            "Unknown"
        };
        let win_station_name = {
            let mut buffer: LPVOID = ptr::null_mut();
            let mut bytes: DWORD = 0;
            let ok = WTSQuerySessionInformationW(
                ptr::null_mut() as HANDLE,
                console_session_id,
                WtsInfoClass::WTSWinStationName as u32,
                &mut buffer as *mut _,
                &mut bytes as *mut _,
            );
            if ok != 0 && !buffer.is_null() && bytes > 1 {
                let len = bytes as usize / 2;
                let slice = std::slice::from_raw_parts(buffer as *const u16, len);
                let nul = slice.iter().position(|c| *c == 0).unwrap_or(len);
                let name = String::from_utf16_lossy(&slice[..nul]);
                WTSFreeMemory(buffer);
                Some(name)
            } else {
                if ok == 0 {
                    let err = GetLastError();
                    debug!(
                        error = err,
                        "launch_capture_helper: WTSQuerySessionInformationW failed"
                    );
                }
                if !buffer.is_null() {
                    WTSFreeMemory(buffer);
                }
                None
            }
        };
        let active_console_session_id = winapi::um::winbase::WTSGetActiveConsoleSessionId();
        let is_non_console_target = is_non_console_capture_target(
            console_session_id,
            active_console_session_id,
            win_station_name.as_deref(),
        );
        let service_pid = GetCurrentProcessId();
        let mut service_session_id: u32 = u32::MAX;
        let service_session_ok =
            ProcessIdToSessionId(service_pid, &mut service_session_id as *mut _) != 0;
        debug!(
            target_session_id = console_session_id,
            target_wts_user = %target_wts_user,
            target_wts_domain = %target_wts_domain,
            target_wts_state = target_wts_state,
            target_wts_state_name = target_wts_state_name,
            active_console_session_id = active_console_session_id,
            target_win_station = win_station_name.as_deref().unwrap_or("<unknown>"),
            target_is_non_console = is_non_console_target,
            service_pid = service_pid,
            service_session_id = service_session_id,
            service_session_ok = service_session_ok,
            "launch_capture_helper: target/session snapshot"
        );
        let probed_input_desktop_name = {
            // Probe the active input desktop from WinSta0, but do not launch
            // the helper onto that desktop directly.
            use winapi::um::winuser::{
                CloseWindowStation, OpenWindowStationW, SetProcessWindowStation, WINSTA_ALL_ACCESS,
            };
            let current_winsta = winapi::um::winuser::GetProcessWindowStation();
            let winsta0_name: Vec<u16> =
                "WinSta0".encode_utf16().chain(std::iter::once(0)).collect();
            let winsta0 = OpenWindowStationW(winsta0_name.as_ptr(), 0, WINSTA_ALL_ACCESS);
            if winsta0.is_null() {
                None
            } else {
                let mut probed = None;
                if SetProcessWindowStation(winsta0) != 0 {
                    probed = crate::control::input_desktop_name();
                    let _ = SetProcessWindowStation(current_winsta);
                }
                CloseWindowStation(winsta0);
                probed
            }
        };
        let launch_plan = resolve_capture_launch_plan(
            is_non_console_target,
            probed_input_desktop_name.as_deref(),
        );
        let desktop_target = launch_plan.desktop_target;
        debug!(
            target_session_id = console_session_id,
            probed_input_desktop = probed_input_desktop_name.as_deref().unwrap_or("<unknown>"),
            desktop_target = desktop_target,
            prefer_system_token = launch_plan.prefer_system_token,
            allow_winlogon_token = launch_plan.allow_winlogon_token,
            "launch_capture_helper: resolved capture launch plan"
        );

        let log_token_diagnostics = |token: HANDLE, label: &str| {
            let mut ret_len: DWORD = 0;
            let mut token_session_id: u32 = u32::MAX;
            let token_session_ok = GetTokenInformation(
                token,
                TokenSessionId,
                &mut token_session_id as *mut _ as *mut _,
                std::mem::size_of::<u32>() as u32,
                &mut ret_len as *mut _,
            );
            let token_session_err = if token_session_ok != 0 {
                0
            } else {
                GetLastError()
            };

            let mut token_type: i32 = -1;
            let token_type_ok = GetTokenInformation(
                token,
                TokenType,
                &mut token_type as *mut _ as *mut _,
                std::mem::size_of::<i32>() as u32,
                &mut ret_len as *mut _,
            );
            let token_type_err = if token_type_ok != 0 {
                0
            } else {
                GetLastError()
            };

            let mut stats: TOKEN_STATISTICS = std::mem::zeroed();
            let token_stats_ok = GetTokenInformation(
                token,
                TokenStatistics,
                &mut stats as *mut _ as *mut _,
                std::mem::size_of::<TOKEN_STATISTICS>() as u32,
                &mut ret_len as *mut _,
            );
            let token_stats_err = if token_stats_ok != 0 {
                0
            } else {
                GetLastError()
            };

            let mut elevation = TOKEN_ELEVATION { TokenIsElevated: 0 };
            let token_elevation_ok = GetTokenInformation(
                token,
                TokenElevation,
                &mut elevation as *mut _ as *mut _,
                std::mem::size_of::<TOKEN_ELEVATION>() as u32,
                &mut ret_len as *mut _,
            );
            let token_elevation_err = if token_elevation_ok != 0 {
                0
            } else {
                GetLastError()
            };

            let mut elevation_type: TOKEN_ELEVATION_TYPE = 0;
            let token_elevation_type_ok = GetTokenInformation(
                token,
                TokenElevationType,
                &mut elevation_type as *mut _ as *mut _,
                std::mem::size_of::<TOKEN_ELEVATION_TYPE>() as u32,
                &mut ret_len as *mut _,
            );
            let token_elevation_type_err = if token_elevation_type_ok != 0 {
                0
            } else {
                GetLastError()
            };

            debug!(
                token_label = label,
                token_session_id = token_session_id,
                token_session_ok = token_session_ok != 0,
                token_session_err = token_session_err,
                token_type = token_type,
                token_type_ok = token_type_ok != 0,
                token_type_err = token_type_err,
                token_auth_id_low = stats.AuthenticationId.LowPart,
                token_auth_id_high = stats.AuthenticationId.HighPart,
                token_stats_ok = token_stats_ok != 0,
                token_stats_err = token_stats_err,
                token_is_elevated = elevation.TokenIsElevated,
                token_elevation_ok = token_elevation_ok != 0,
                token_elevation_err = token_elevation_err,
                token_elevation_type = elevation_type,
                token_elevation_type_ok = token_elevation_type_ok != 0,
                token_elevation_type_err = token_elevation_type_err,
                "launch_capture_helper: token diagnostics"
            );
        };

        let enable_privilege = |name: *const u16, label: &str| {
            let mut token = winapi::um::winnt::HANDLE::default();
            if OpenProcessToken(
                GetCurrentProcess(),
                TOKEN_QUERY | TOKEN_ADJUST_DEFAULT,
                &mut token,
            ) == 0
            {
                let err = GetLastError();
                debug!(
                    error = err,
                    privilege = label,
                    "launch_capture_helper: OpenProcessToken for privilege failed"
                );
                return;
            }
            let mut luid = winapi::shared::ntdef::LUID {
                LowPart: 0,
                HighPart: 0,
            };
            if LookupPrivilegeValueW(std::ptr::null(), name, &mut luid) == 0 {
                let err = GetLastError();
                debug!(
                    error = err,
                    privilege = label,
                    "launch_capture_helper: LookupPrivilegeValueW failed"
                );
                let _ = CloseHandle(token);
                return;
            }
            let mut tp: TOKEN_PRIVILEGES = std::mem::zeroed();
            tp.PrivilegeCount = 1;
            tp.Privileges[0].Luid = luid;
            tp.Privileges[0].Attributes = winapi::um::winnt::SE_PRIVILEGE_ENABLED;
            if AdjustTokenPrivileges(
                token,
                FALSE,
                &mut tp,
                0,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            ) == 0
            {
                let err = GetLastError();
                debug!(
                    error = err,
                    privilege = label,
                    "launch_capture_helper: AdjustTokenPrivileges failed"
                );
            } else {
                let err = GetLastError();
                if err == 0 {
                    debug!(
                        privilege = label,
                        "launch_capture_helper: privilege enabled"
                    );
                } else {
                    debug!(
                        error = err,
                        privilege = label,
                        "launch_capture_helper: privilege enable reported warning"
                    );
                }
            }
            let _ = CloseHandle(token);
        };

        let se_assign: Vec<u16> = "SeAssignPrimaryTokenPrivilege"
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        let se_quota: Vec<u16> = "SeIncreaseQuotaPrivilege"
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        let se_tcb: Vec<u16> = "SeTcbPrivilege"
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        let se_debug: Vec<u16> = "SeDebugPrivilege"
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        enable_privilege(se_assign.as_ptr(), "SeAssignPrimaryTokenPrivilege");
        enable_privilege(se_quota.as_ptr(), "SeIncreaseQuotaPrivilege");
        enable_privilege(se_tcb.as_ptr(), "SeTcbPrivilege");
        enable_privilege(se_debug.as_ptr(), "SeDebugPrivilege");

        let mut h_service_token = winapi::um::winnt::HANDLE::default();
        let service_token_ok = OpenProcessToken(
            GetCurrentProcess(),
            TOKEN_QUERY
                | TOKEN_DUPLICATE
                | TOKEN_ASSIGN_PRIMARY
                | TOKEN_ADJUST_DEFAULT
                | TOKEN_ADJUST_SESSIONID,
            &mut h_service_token,
        );
        if service_token_ok != 0 {
            log_token_diagnostics(h_service_token, "service_process_token");
            let _ = CloseHandle(h_service_token);
        } else {
            debug!(
                error = GetLastError(),
                "launch_capture_helper: failed to open service process token for diagnostics"
            );
        }

        let try_create_with_token = |token: winapi::um::winnt::HANDLE,
                                     label: &str|
         -> Result<LaunchedCaptureHelper, u32> {
            log_token_diagnostics(token, label);
            let mut elevation = TOKEN_ELEVATION { TokenIsElevated: 0 };
            let mut ret_len: DWORD = 0;
            let _ = GetTokenInformation(
                token,
                TokenElevation,
                &mut elevation as *mut _ as *mut _,
                std::mem::size_of::<TOKEN_ELEVATION>() as u32,
                &mut ret_len as *mut _,
            );
            let mut elevation_type: TOKEN_ELEVATION_TYPE = 0;
            let _ = GetTokenInformation(
                token,
                TokenElevationType,
                &mut elevation_type as *mut _ as *mut _,
                std::mem::size_of::<TOKEN_ELEVATION_TYPE>() as u32,
                &mut ret_len as *mut _,
            );
            debug!(
                label = label,
                is_elevated = elevation.TokenIsElevated,
                elevation_type = elevation_type,
                "launch_capture_helper: token elevation state"
            );
            let uiaccess_value: DWORD = 1;
            let set_ui_access_ok = winapi::um::securitybaseapi::SetTokenInformation(
                token,
                winapi::um::winnt::TokenUIAccess,
                &uiaccess_value as *const _ as *mut _,
                std::mem::size_of::<DWORD>() as u32,
            );
            debug!(
                label = label,
                ui_access_set_ok = set_ui_access_ok != 0,
                ui_access_set_error = if set_ui_access_ok != 0 {
                    0
                } else {
                    GetLastError()
                },
                "launch_capture_helper: token ui-access toggle"
            );

            let launch_with_cmd = |cmd_wide: &Vec<u16>,
                                   launch_role: &str,
                                   desktop_name: &str|
             -> Result<LaunchedCaptureHelper, u32> {
                let mut si: winapi::um::processthreadsapi::STARTUPINFOW = std::mem::zeroed();
                si.cb = std::mem::size_of::<winapi::um::processthreadsapi::STARTUPINFOW>() as u32;
                let desktop_wide: Vec<u16> = desktop_name
                    .encode_utf16()
                    .chain(std::iter::once(0))
                    .collect();
                si.lpDesktop = desktop_wide.as_ptr() as *mut u16;
                let mut pi: winapi::um::processthreadsapi::PROCESS_INFORMATION = std::mem::zeroed();
                let cmd_preview = if cmd_wide.is_empty() {
                    String::from("<empty>")
                } else {
                    String::from_utf16_lossy(&cmd_wide[..cmd_wide.len().saturating_sub(1)])
                };
                debug!(
                    label = label,
                    launch_role = launch_role,
                    target_session_id = console_session_id,
                    desktop_target = desktop_name,
                    command_line = %cmd_preview,
                    "launch_capture_helper: CreateProcessAsUserW attempt"
                );
                let ok = winapi::um::processthreadsapi::CreateProcessAsUserW(
                    token,
                    ptr::null(),
                    cmd_wide.as_ptr() as *mut u16,
                    ptr::null_mut(),
                    ptr::null_mut(),
                    FALSE,
                    CREATE_NO_WINDOW | ABOVE_NORMAL_PRIORITY_CLASS,
                    ptr::null_mut(),
                    ptr::null(),
                    &mut si,
                    &mut pi,
                );
                if ok != 0 {
                    let mut child_session_id: u32 = u32::MAX;
                    let child_session_ok =
                        ProcessIdToSessionId(pi.dwProcessId, &mut child_session_id as *mut _) != 0;
                    debug!(
                        label = label,
                        launch_role = launch_role,
                        child_pid = pi.dwProcessId,
                        child_tid = pi.dwThreadId,
                        child_session_id = child_session_id,
                        child_session_ok = child_session_ok,
                        desktop_target = desktop_name,
                        "launch_capture_helper: CreateProcessAsUserW success"
                    );
                    let _ = CloseHandle(pi.hThread);
                    return Ok(LaunchedCaptureHelper {
                        process_handle: pi.hProcess as usize,
                        process_id: pi.dwProcessId,
                    });
                }
                let err = GetLastError();
                debug!(
                    label = label,
                    launch_role = launch_role,
                    error = err,
                    desktop_target = desktop_name,
                    "launch_capture_helper: CreateProcessAsUserW failure"
                );
                Err(err)
            };

            let err = match launch_with_cmd(&cmd_capture_wide, "capture", desktop_target) {
                Ok(helper) => return Ok(helper),
                Err(err) => err,
            };
            if is_non_console_target && desktop_target.eq_ignore_ascii_case("winsta0\\default") {
                debug!(
                    label = label,
                    error = err,
                    target_session_id = console_session_id,
                    "launch_capture_helper: retrying CreateProcessAsUserW on non-console winlogon desktop"
                );
                let err2 = match launch_with_cmd(&cmd_capture_wide, "capture", "winsta0\\winlogon")
                {
                    Ok(helper) => return Ok(helper),
                    Err(err) => err,
                };
                debug!(
                    error = err2,
                    label = label,
                    target_session_id = console_session_id,
                    "launch_capture_helper: non-console winlogon desktop fallback failed"
                );
            }
            if err == 740 {
                let mut linked: TOKEN_LINKED_TOKEN = std::mem::zeroed();
                let ok_linked = GetTokenInformation(
                    token,
                    TokenLinkedToken,
                    &mut linked as *mut _ as *mut _,
                    std::mem::size_of::<TOKEN_LINKED_TOKEN>() as u32,
                    &mut ret_len as *mut _,
                );
                if ok_linked != 0 {
                    debug!(
                        label = label,
                        "launch_capture_helper: retrying CreateProcessAsUserW with linked token"
                    );
                    let err2 = launch_with_cmd(&cmd_capture_wide, "capture", desktop_target)
                        .err()
                        .unwrap_or(0);
                    let _ = CloseHandle(linked.LinkedToken);
                    debug!(
                        error = err2,
                        label = label,
                        "launch_capture_helper: linked token CreateProcessAsUserW failed"
                    );
                    return Err(err2);
                }
            }
            debug!(
                error = err,
                label = label,
                "launch_capture_helper: CreateProcessAsUserW failed"
            );
            Err(err)
        };

        let try_launch_with_winlogon_token = || -> Result<Option<LaunchedCaptureHelper>, u32> {
            if !launch_plan.allow_winlogon_token {
                return Ok(None);
            }
            let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
            if snapshot == INVALID_HANDLE_VALUE {
                return Err(GetLastError());
            }
            let mut entry: PROCESSENTRY32W = std::mem::zeroed();
            entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
            let mut winlogon_pid: Option<u32> = None;
            if Process32FirstW(snapshot, &mut entry) != 0 {
                loop {
                    let exe = String::from_utf16_lossy(&entry.szExeFile);
                    let exe = exe.trim_end_matches('\0');
                    if exe.eq_ignore_ascii_case("winlogon.exe") {
                        let mut sid = 0u32;
                        if ProcessIdToSessionId(entry.th32ProcessID, &mut sid) != 0
                            && sid == console_session_id
                        {
                            winlogon_pid = Some(entry.th32ProcessID);
                            break;
                        }
                    }
                    if Process32NextW(snapshot, &mut entry) == 0 {
                        break;
                    }
                }
            }
            let _ = CloseHandle(snapshot);
            let Some(pid) = winlogon_pid else {
                return Err(2);
            };
            debug!(
                target_session_id = console_session_id,
                winlogon_pid = pid,
                "launch_capture_helper: winlogon process selected for token duplication"
            );
            let h_proc = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, FALSE, pid);
            if h_proc.is_null() {
                return Err(GetLastError());
            }
            let mut h_token = winapi::um::winnt::HANDLE::default();
            if OpenProcessToken(
                h_proc,
                TOKEN_DUPLICATE | TOKEN_ASSIGN_PRIMARY | TOKEN_QUERY,
                &mut h_token,
            ) == 0
            {
                let err = GetLastError();
                let _ = CloseHandle(h_proc);
                return Err(err);
            }
            let mut h_primary = winapi::um::winnt::HANDLE::default();
            let dup_ok = DuplicateTokenEx(
                h_token,
                MAXIMUM_ALLOWED,
                ptr::null_mut(),
                SecurityImpersonation,
                TokenPrimary,
                &mut h_primary,
            );
            let _ = CloseHandle(h_token);
            let _ = CloseHandle(h_proc);
            if dup_ok == 0 {
                return Err(GetLastError());
            }
            let mut target_session = console_session_id;
            log_token_diagnostics(h_primary, "winlogon_primary_before_set_session");
            let set_ok = SetTokenInformation(
                h_primary,
                TokenSessionId,
                &mut target_session as *mut _ as *mut _,
                std::mem::size_of::<u32>() as u32,
            );
            debug!(
                token_label = "winlogon_primary",
                target_session_id = target_session,
                set_token_session_ok = set_ok != 0,
                set_token_session_error = if set_ok != 0 { 0 } else { GetLastError() },
                "launch_capture_helper: SetTokenInformation(TokenSessionId)"
            );
            log_token_diagnostics(h_primary, "winlogon_primary_after_set_session");
            let result = try_create_with_token(h_primary, "winlogon");
            let _ = CloseHandle(h_primary);
            result.map(Some)
        };

        let try_launch_with_system_token = || -> Result<Option<LaunchedCaptureHelper>, u32> {
            // Duplicate LocalSystem token, set target session, and launch helper.
            let mut h_proc_token = winapi::um::winnt::HANDLE::default();
            let access = TOKEN_ASSIGN_PRIMARY
                | TOKEN_DUPLICATE
                | TOKEN_QUERY
                | TOKEN_ADJUST_DEFAULT
                | TOKEN_ADJUST_SESSIONID;
            let ok = winapi::um::processthreadsapi::OpenProcessToken(
                GetCurrentProcess(),
                access,
                &mut h_proc_token,
            );
            if ok == 0 {
                let err = GetLastError();
                debug!(
                    error = err,
                    "launch_capture_helper: LocalSystem OpenProcessToken failed"
                );
                return Err(err);
            }
            log_token_diagnostics(h_proc_token, "system_process_token");

            let mut h_primary = winapi::um::winnt::HANDLE::default();
            let dup_ok = DuplicateTokenEx(
                h_proc_token,
                MAXIMUM_ALLOWED,
                ptr::null_mut(),
                SecurityImpersonation,
                TokenPrimary,
                &mut h_primary,
            );
            let _ = CloseHandle(h_proc_token);
            if dup_ok == 0 {
                let err = GetLastError();
                debug!(
                    error = err,
                    "launch_capture_helper: LocalSystem DuplicateTokenEx failed"
                );
                return Err(err);
            }
            log_token_diagnostics(h_primary, "system_primary_before_set_session");

            let mut target_session = console_session_id;
            let set_ok = SetTokenInformation(
                h_primary,
                TokenSessionId,
                &mut target_session as *mut _ as *mut _,
                std::mem::size_of::<u32>() as u32,
            );
            if set_ok == 0 {
                let err = GetLastError();
                debug!(
                    error = err,
                    "launch_capture_helper: LocalSystem SetTokenInformation(TokenSessionId) failed"
                );
                let _ = CloseHandle(h_primary);
                return Err(err);
            }
            debug!(
                token_label = "system_primary",
                target_session_id = target_session,
                set_token_session_ok = true,
                "launch_capture_helper: SetTokenInformation(TokenSessionId)"
            );
            log_token_diagnostics(h_primary, "system_primary_after_set_session");

            let result = try_create_with_token(h_primary, "system");
            let _ = CloseHandle(h_primary);
            result.map(Some)
        };

        debug!(
            target_session_id = console_session_id,
            active_console_session_id = active_console_session_id,
            prefer_user_token_first = !launch_plan.prefer_system_token,
            prefer_system_token_first = launch_plan.prefer_system_token,
            allow_winlogon_path = launch_plan.allow_winlogon_token,
            "launch_capture_helper: token strategy decision"
        );

        if launch_plan.prefer_system_token {
            debug!(
                session_id = console_session_id,
                probed_input_desktop = probed_input_desktop_name.as_deref().unwrap_or("<unknown>"),
                "launch_capture_helper: attempting LocalSystem token path first for secure input desktop"
            );
            match try_launch_with_system_token() {
                Ok(Some(helper)) => {
                    debug!(
                        session_id = console_session_id,
                        "launch_capture_helper: LocalSystem token preferred path succeeded"
                    );
                    return Some(helper);
                }
                Err(err) => {
                    debug!(
                        error = err,
                        session_id = console_session_id,
                        "launch_capture_helper: LocalSystem token preferred path failed"
                    );
                }
                Ok(None) => {}
            }
        }

        debug!(
            target_session_id = console_session_id,
            target_is_non_console = is_non_console_target,
            "launch_capture_helper: attempting WTS user token path"
        );
        let mut h_token = winapi::um::winnt::HANDLE::default();
        if WTSQueryUserToken(console_session_id, &mut h_token) == 0 {
            let err = GetLastError();
            let err_note = match err {
                1008 => "ERROR_NO_SUCH_LOGON_SESSION: session has no user token (disconnected or no one logged in; common when only RDP is used)",
                1314 => "ERROR_PRIVILEGE_NOT_HELD: must run as LocalSystem",
                _ => "",
            };
            debug!(
                error = err,
                error_note = %err_note,
                session_id = console_session_id,
                    "launch_capture_helper: WTSQueryUserToken failed"
            );
            if err == 1008 && !launch_plan.prefer_system_token {
                debug!(
                    session_id = console_session_id,
                    target_is_non_console = is_non_console_target,
                    "launch_capture_helper: attempting LocalSystem token fallback"
                );
                match try_launch_with_system_token() {
                    Ok(Some(helper)) => {
                        debug!(
                            session_id = console_session_id,
                            "launch_capture_helper: LocalSystem token fallback succeeded"
                        );
                        return Some(helper);
                    }
                    Err(fallback_err) => {
                        debug!(
                            error = fallback_err,
                            session_id = console_session_id,
                            "launch_capture_helper: LocalSystem token fallback failed"
                        );
                    }
                    Ok(None) => {}
                }
            }
            if launch_plan.allow_winlogon_token {
                debug!(
                    session_id = console_session_id,
                    error = err,
                    "launch_capture_helper: attempting winlogon token fallback as last resort"
                );
                match try_launch_with_winlogon_token() {
                    Ok(Some(helper)) => {
                        debug!(
                            session_id = console_session_id,
                            "launch_capture_helper: winlogon token fallback succeeded"
                        );
                        return Some(helper);
                    }
                    Err(fallback_err) => {
                        debug!(
                            session_id = console_session_id,
                            error = fallback_err,
                            "launch_capture_helper: winlogon token fallback failed"
                        );
                    }
                    Ok(None) => {}
                }
            }
            return None;
        }
        log_token_diagnostics(h_token, "wts_user_token");

        let mut h_primary = winapi::um::winnt::HANDLE::default();
        let dup_ok = DuplicateTokenEx(
            h_token,
            MAXIMUM_ALLOWED,
            ptr::null_mut(),
            SecurityImpersonation,
            TokenPrimary,
            &mut h_primary,
        );
        let _ = CloseHandle(h_token);
        if dup_ok == 0 {
            let err = GetLastError();
            debug!(
                error = err,
                "launch_capture_helper: DuplicateTokenEx failed"
            );
            return None;
        }
        log_token_diagnostics(h_primary, "wts_primary_token");

        let launched_helper = try_create_with_token(h_primary, "primary");
        let _ = CloseHandle(h_primary);
        if launched_helper.is_err() {
            let err = launched_helper.err().unwrap_or(0);
            debug!(
                error = err,
                "launch_capture_helper: CreateProcessAsUserW failed"
            );
            if launch_plan.allow_winlogon_token {
                debug!(
                    session_id = console_session_id,
                    error = err,
                    "launch_capture_helper: attempting winlogon token fallback after user-token launch failure"
                );
                match try_launch_with_winlogon_token() {
                    Ok(Some(helper)) => {
                        debug!(
                            session_id = console_session_id,
                            "launch_capture_helper: winlogon token fallback succeeded"
                        );
                        return Some(helper);
                    }
                    Err(fallback_err) => {
                        debug!(
                            session_id = console_session_id,
                            error = fallback_err,
                            "launch_capture_helper: winlogon token fallback failed"
                        );
                    }
                    Ok(None) => {}
                }
            }
            return None;
        }
        debug!(
            target_session_id = console_session_id,
            "launch_capture_helper: helper process started via WTS user token path"
        );
        launched_helper.ok()
    }
}

#[cfg(test)]
mod tests {
    use super::{is_non_console_capture_target, resolve_capture_launch_plan};

    #[test]
    fn active_console_session_is_treated_as_console() {
        assert!(!is_non_console_capture_target(6, 6, Some("Console")));
    }

    #[test]
    fn console_winstation_overrides_invalid_active_console_id() {
        assert!(!is_non_console_capture_target(6, u32::MAX, Some("Console")));
    }

    #[test]
    fn non_console_session_remains_non_console() {
        assert!(is_non_console_capture_target(8, 6, Some("RDP-Tcp#5")));
    }

    #[test]
    fn console_winlogon_probe_keeps_default_desktop_and_prefers_privileged_launch() {
        let plan = resolve_capture_launch_plan(false, Some("Winlogon"));
        assert_eq!(plan.desktop_target, "winsta0\\default");
        assert!(plan.prefer_system_token);
        assert!(plan.allow_winlogon_token);
    }

    #[test]
    fn console_default_probe_stays_on_default_desktop_without_privileged_launch() {
        let plan = resolve_capture_launch_plan(false, Some("Default"));
        assert_eq!(plan.desktop_target, "winsta0\\default");
        assert!(!plan.prefer_system_token);
        assert!(!plan.allow_winlogon_token);
    }
}
