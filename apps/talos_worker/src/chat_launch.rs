//! Launch `talos_worker_chat` in an interactive user desktop/session.

#[cfg(target_os = "windows")]
use std::ffi::OsStr;
#[cfg(target_os = "windows")]
use std::os::windows::ffi::OsStrExt;
#[cfg(any(target_os = "windows", target_os = "macos"))]
use std::path::Path;
#[cfg(target_os = "windows")]
use std::ptr::{null, null_mut};

#[cfg(target_os = "macos")]
use anyhow::Context;
#[cfg(any(target_os = "windows", target_os = "macos"))]
use anyhow::Result;
#[cfg(any(target_os = "windows", target_os = "macos"))]
use tracing::{debug, info, warn};
#[cfg(target_os = "windows")]
use winapi::ctypes::c_void;
#[cfg(target_os = "windows")]
use winapi::shared::minwindef::{BOOL, DWORD, FALSE};
#[cfg(target_os = "windows")]
use winapi::um::handleapi::CloseHandle;
#[cfg(target_os = "windows")]
use winapi::um::processthreadsapi::{CreateProcessAsUserW, PROCESS_INFORMATION, STARTUPINFOW};
#[cfg(target_os = "windows")]
use winapi::um::securitybaseapi::DuplicateTokenEx as DuplicateTokenExSecurity;
#[cfg(target_os = "windows")]
use winapi::um::userenv::{CreateEnvironmentBlock, DestroyEnvironmentBlock};
#[cfg(target_os = "windows")]
use winapi::um::winbase::{WTSGetActiveConsoleSessionId, CREATE_UNICODE_ENVIRONMENT};
#[cfg(target_os = "windows")]
use winapi::um::winnt::{SecurityImpersonation, TokenPrimary, HANDLE, MAXIMUM_ALLOWED};
#[cfg(target_os = "windows")]
use winapi::um::wtsapi32::WTSQueryUserToken;

#[cfg(target_os = "windows")]
#[repr(C)]
struct WtsSessionInfoW {
    session_id: DWORD,
    win_station_name: *mut u16,
    state: i32,
}

#[cfg(target_os = "windows")]
#[link(name = "wtsapi32")]
extern "system" {
    fn WTSEnumerateSessionsW(
        h_server: HANDLE,
        reserved: DWORD,
        version: DWORD,
        session_info: *mut *mut WtsSessionInfoW,
        count: *mut DWORD,
    ) -> BOOL;
    fn WTSFreeMemory(memory: *mut c_void);
}

#[cfg(target_os = "windows")]
const WTS_ACTIVE: i32 = 0;

#[derive(Debug, Clone)]
pub struct RebootNoticeLaunchConfig {
    pub notice_id: String,
    pub deadline_unix_ms: u64,
    pub deferrals_used: u32,
    pub max_deferrals: u32,
    pub delay_minutes: u32,
}

#[derive(Debug, Clone)]
pub struct AiApprovalLaunchConfig {
    pub approval_id: String,
    pub requester_label: String,
    pub requester_email: Option<String>,
    pub organization_name: Option<String>,
    pub device_label: String,
    pub reason: String,
    pub expires_at_unix_ms: u64,
    pub approval_window_expires_at_unix_ms: u64,
}

#[derive(Debug, Clone, Copy)]
pub struct NoInteractiveUserError;

impl std::fmt::Display for NoInteractiveUserError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "no interactive user is currently logged in")
    }
}

impl std::error::Error for NoInteractiveUserError {}

pub fn is_no_interactive_user_error(error: &anyhow::Error) -> bool {
    error.downcast_ref::<NoInteractiveUserError>().is_some()
}

pub fn build_chat_ui_args(port: u16, bridge_secret: &str) -> Vec<String> {
    vec![
        format!("--local-port={port}"),
        format!("--bridge-secret={bridge_secret}"),
    ]
}

pub fn build_update_reboot_notice_args(
    port: u16,
    bridge_secret: &str,
    config: &RebootNoticeLaunchConfig,
) -> Vec<String> {
    vec![
        "--mode=update-reboot".to_string(),
        format!("--local-port={port}"),
        format!("--bridge-secret={bridge_secret}"),
        format!("--notice-id={}", config.notice_id),
        format!("--deadline-unix-ms={}", config.deadline_unix_ms),
        format!("--deferrals-used={}", config.deferrals_used),
        format!("--max-deferrals={}", config.max_deferrals),
        format!("--delay-minutes={}", config.delay_minutes),
    ]
}

pub fn build_ai_approval_args(
    port: u16,
    bridge_secret: &str,
    config: &AiApprovalLaunchConfig,
) -> Vec<String> {
    let mut args = vec![
        "--mode=ai-approval".to_string(),
        format!("--local-port={port}"),
        format!("--bridge-secret={bridge_secret}"),
        format!("--approval-id={}", config.approval_id),
        format!("--requester-label={}", config.requester_label),
        format!("--device-label={}", config.device_label),
        format!("--reason={}", config.reason),
        format!("--expires-at-unix-ms={}", config.expires_at_unix_ms),
        format!(
            "--approval-window-expires-at-unix-ms={}",
            config.approval_window_expires_at_unix_ms
        ),
    ];
    if let Some(email) = config
        .requester_email
        .as_ref()
        .filter(|value| !value.trim().is_empty())
    {
        args.push(format!("--requester-email={email}"));
    }
    if let Some(name) = config
        .organization_name
        .as_ref()
        .filter(|value| !value.trim().is_empty())
    {
        args.push(format!("--organization-name={name}"));
    }
    args
}

#[cfg(target_os = "windows")]
pub fn worker_chat_exe_path() -> std::path::PathBuf {
    let dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()));
    if let Some(dir) = dir {
        let preferred = dir.join("talos_worker_chat.exe");
        if preferred.exists() {
            return preferred;
        }
        return dir.join("talos_worker_chat.exe");
    }
    std::path::PathBuf::from("talos_worker_chat.exe")
}

#[cfg(target_os = "macos")]
pub fn worker_chat_exe_path() -> std::path::PathBuf {
    std::path::PathBuf::from(
        "/Library/Talos/Worker/Talos Worker Chat.app/Contents/MacOS/talos_worker_chat",
    )
}

#[cfg(target_os = "windows")]
fn wide_null(s: &str) -> Vec<u16> {
    OsStr::new(s)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

#[cfg(target_os = "windows")]
fn read_environment_block(block: *mut c_void) -> Vec<String> {
    if block.is_null() {
        return Vec::new();
    }

    let mut entries = Vec::new();
    let mut cursor = block as *const u16;
    unsafe {
        loop {
            if *cursor == 0 {
                break;
            }
            let start = cursor;
            let mut len = 0usize;
            while *cursor != 0 {
                len += 1;
                cursor = cursor.add(1);
            }
            entries.push(String::from_utf16_lossy(std::slice::from_raw_parts(
                start, len,
            )));
            cursor = cursor.add(1);
        }
    }
    entries
}

#[cfg(target_os = "windows")]
fn env_value(entries: &[String], name: &str) -> Option<String> {
    let prefix = format!("{name}=");
    entries
        .iter()
        .find_map(|entry| entry.strip_prefix(&prefix).map(|value| value.to_string()))
}

#[cfg(target_os = "windows")]
fn set_env_entry(entries: &mut Vec<String>, name: &str, value: String) {
    let prefix = format!("{name}=");
    if let Some(existing) = entries.iter_mut().find(|entry| {
        entry
            .to_ascii_uppercase()
            .starts_with(&prefix.to_ascii_uppercase())
    }) {
        *existing = format!("{name}={value}");
    } else {
        entries.push(format!("{name}={value}"));
    }
}

#[cfg(target_os = "windows")]
fn environment_block_to_wide(entries: &[String]) -> Vec<u16> {
    let mut block = Vec::new();
    for entry in entries {
        block.extend(OsStr::new(entry).encode_wide());
        block.push(0);
    }
    block.push(0);
    block
}

#[cfg(target_os = "windows")]
fn build_chat_environment(primary_token: HANDLE) -> (Vec<u16>, Option<String>) {
    let mut raw_env: *mut c_void = null_mut();
    let created = unsafe { CreateEnvironmentBlock(&mut raw_env, primary_token, FALSE) } != 0;
    let mut entries = if created {
        read_environment_block(raw_env)
    } else {
        Vec::new()
    };
    if created {
        unsafe {
            let _ = DestroyEnvironmentBlock(raw_env);
        }
    } else {
        let os_error = std::io::Error::last_os_error();
        warn!(
            target: "rmm_chat",
            error = %os_error,
            "CreateEnvironmentBlock failed for talos_worker_chat; using minimal inherited environment"
        );
    }

    for name in ["SystemRoot", "WINDIR", "TEMP", "TMP", "PATH"] {
        if env_value(&entries, name).is_none() {
            if let Ok(value) = std::env::var(name) {
                set_env_entry(&mut entries, name, value);
            }
        }
    }

    let webview_user_data_dir = env_value(&entries, "LOCALAPPDATA")
        .map(|base| {
            Path::new(&base)
                .join("Talos")
                .join("TalosWorkerChat")
                .join("WebView2")
        })
        .or_else(|| {
            std::env::var("PROGRAMDATA").ok().map(|base| {
                Path::new(&base)
                    .join("Talos")
                    .join("TalosWorkerChat")
                    .join("WebView2")
            })
        })
        .unwrap_or_else(|| {
            std::env::temp_dir()
                .join("Talos")
                .join("TalosWorkerChat")
                .join("WebView2")
        });
    let _ = std::fs::create_dir_all(&webview_user_data_dir);
    let webview_user_data_dir = webview_user_data_dir.to_string_lossy().to_string();
    set_env_entry(
        &mut entries,
        "WEBVIEW2_USER_DATA_FOLDER",
        webview_user_data_dir.clone(),
    );

    info!(
        target: "rmm_chat",
        has_user_environment = created,
        webview_user_data_dir = %webview_user_data_dir,
        "prepared talos_worker_chat environment"
    );

    (
        environment_block_to_wide(&entries),
        Some(webview_user_data_dir),
    )
}

#[cfg(target_os = "windows")]
fn quote_windows_command_arg(arg: &str) -> String {
    if !arg.is_empty()
        && !arg
            .bytes()
            .any(|b| matches!(b, b' ' | b'\t' | b'\n' | b'\r' | b'"'))
    {
        return arg.to_string();
    }

    let mut quoted = String::from("\"");
    let mut backslashes = 0usize;
    for ch in arg.chars() {
        if ch == '\\' {
            backslashes += 1;
            continue;
        }
        if ch == '"' {
            quoted.push_str(&"\\".repeat(backslashes * 2 + 1));
            quoted.push('"');
        } else {
            quoted.push_str(&"\\".repeat(backslashes));
            quoted.push(ch);
        }
        backslashes = 0;
    }
    quoted.push_str(&"\\".repeat(backslashes * 2));
    quoted.push('"');
    quoted
}

#[cfg(target_os = "windows")]
fn build_windows_command_line(exe: &Path, args: &[String]) -> String {
    std::iter::once(exe.to_string_lossy().to_string())
        .chain(args.iter().cloned())
        .map(|arg| quote_windows_command_arg(&arg))
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(target_os = "windows")]
fn valid_wts_session_id(session_id: u32) -> bool {
    session_id != 0 && session_id != 0xFFFFFFFF
}

#[cfg(target_os = "windows")]
fn active_wts_session_ids() -> Vec<u32> {
    let mut session_info: *mut WtsSessionInfoW = null_mut();
    let mut count: DWORD = 0;
    let ok = unsafe {
        WTSEnumerateSessionsW(
            null_mut(),
            0,
            1,
            &mut session_info as *mut *mut WtsSessionInfoW,
            &mut count as *mut DWORD,
        )
    };
    if ok == 0 || session_info.is_null() || count == 0 {
        let os_error = std::io::Error::last_os_error();
        warn!(
            target: "rmm_chat",
            error = %os_error,
            "WTSEnumerateSessionsW failed while locating chat UI session"
        );
        return Vec::new();
    }

    let sessions = unsafe { std::slice::from_raw_parts(session_info, count as usize) };
    let ids = sessions
        .iter()
        .filter(|session| session.state == WTS_ACTIVE && valid_wts_session_id(session.session_id))
        .map(|session| session.session_id)
        .collect::<Vec<_>>();
    unsafe {
        WTSFreeMemory(session_info as *mut c_void);
    }
    ids
}

#[cfg(target_os = "windows")]
fn wts_user_token_for_session(session_id: u32) -> std::result::Result<HANDLE, std::io::Error> {
    let mut user_token: HANDLE = null_mut();
    let token_ok = unsafe { WTSQueryUserToken(session_id as DWORD, &mut user_token) };
    if token_ok == 0 || user_token.is_null() {
        return Err(std::io::Error::last_os_error());
    }
    Ok(user_token)
}

#[cfg(target_os = "windows")]
fn is_no_user_token_error(error: &std::io::Error) -> bool {
    const ERROR_NO_TOKEN: i32 = 1008;
    error.raw_os_error() == Some(ERROR_NO_TOKEN)
}

#[cfg(target_os = "windows")]
fn resolve_wts_user_token(preferred_session_id: u32) -> Result<(u32, HANDLE)> {
    match wts_user_token_for_session(preferred_session_id) {
        Ok(token) => return Ok((preferred_session_id, token)),
        Err(preferred_error) => {
            let active_console = unsafe { WTSGetActiveConsoleSessionId() };
            warn!(
                target: "rmm_chat",
                session_id = preferred_session_id,
                active_console,
                error = %preferred_error,
                "WTSQueryUserToken failed for preferred chat UI session; searching active user sessions"
            );
            let mut candidates = Vec::new();
            if valid_wts_session_id(active_console) && active_console != preferred_session_id {
                candidates.push(active_console);
            }
            for session_id in active_wts_session_ids() {
                if session_id != preferred_session_id && !candidates.contains(&session_id) {
                    candidates.push(session_id);
                }
            }
            let mut only_no_user_token_errors = is_no_user_token_error(&preferred_error);
            for session_id in candidates {
                match wts_user_token_for_session(session_id) {
                    Ok(token) => {
                        info!(
                            target: "rmm_chat",
                            preferred_session_id,
                            fallback_session_id = session_id,
                            "using fallback active user session for chat UI"
                        );
                        return Ok((session_id, token));
                    }
                    Err(error) => {
                        if !is_no_user_token_error(&error) {
                            only_no_user_token_errors = false;
                        }
                        warn!(
                            target: "rmm_chat",
                            session_id,
                            error = %error,
                            "WTSQueryUserToken failed for fallback chat UI session"
                        );
                    }
                }
            }
            if only_no_user_token_errors {
                return Err(NoInteractiveUserError.into());
            }
            Err(anyhow::anyhow!(
                "WTSQueryUserToken failed for chat UI: {preferred_error}"
            ))
        }
    }
}

/// Spawn the chat UI using the token for `target_session_id` (WTS session).
#[cfg(target_os = "windows")]
pub fn launch_chat_ui(
    target_session_id: u32,
    exe: &Path,
    port: u16,
    bridge_secret: &str,
) -> Result<()> {
    launch_chat_ui_with_args(
        target_session_id,
        exe,
        &build_chat_ui_args(port, bridge_secret),
        port,
        "remote_chat",
    )
}

#[cfg(target_os = "windows")]
pub fn launch_update_reboot_notice_ui(
    target_session_id: u32,
    exe: &Path,
    port: u16,
    bridge_secret: &str,
    config: &RebootNoticeLaunchConfig,
) -> Result<()> {
    launch_chat_ui_with_args(
        target_session_id,
        exe,
        &build_update_reboot_notice_args(port, bridge_secret, config),
        port,
        "update_reboot_notice",
    )
}

#[cfg(target_os = "windows")]
pub fn launch_ai_approval_ui(
    target_session_id: u32,
    exe: &Path,
    port: u16,
    bridge_secret: &str,
    config: &AiApprovalLaunchConfig,
) -> Result<()> {
    launch_chat_ui_with_args(
        target_session_id,
        exe,
        &build_ai_approval_args(port, bridge_secret, config),
        port,
        "ai_approval",
    )
}

#[cfg(target_os = "windows")]
fn launch_chat_ui_with_args(
    target_session_id: u32,
    exe: &Path,
    args: &[String],
    port: u16,
    mode: &str,
) -> Result<()> {
    const CHAT_LOG_TARGET: &str = "rmm_chat";

    debug!(
        target: CHAT_LOG_TARGET,
        target_session_id,
        exe = %exe.display(),
        port,
        mode,
        "launch_chat_ui requested"
    );

    if !valid_wts_session_id(target_session_id) {
        let active = unsafe { WTSGetActiveConsoleSessionId() };
        debug!(
            target: CHAT_LOG_TARGET,
            target_session_id,
            active_console = active,
            "launch_chat_ui resolving invalid target session"
        );
        if valid_wts_session_id(active) {
            return launch_chat_ui_with_args(active, exe, args, port, mode);
        }
        return Err(NoInteractiveUserError.into());
    }

    let (launch_session_id, user_token) = resolve_wts_user_token(target_session_id)?;

    let mut primary_token: HANDLE = null_mut();
    let dup_ok = unsafe {
        DuplicateTokenExSecurity(
            user_token,
            MAXIMUM_ALLOWED,
            null_mut(),
            SecurityImpersonation,
            TokenPrimary,
            &mut primary_token,
        )
    };
    unsafe { CloseHandle(user_token) };
    if dup_ok == 0 || primary_token.is_null() {
        let os_error = std::io::Error::last_os_error();
        warn!(
            target: CHAT_LOG_TARGET,
            target_session_id = launch_session_id,
            error = %os_error,
            "DuplicateTokenEx failed for chat UI token"
        );
        anyhow::bail!("DuplicateTokenEx failed for chat UI token: {os_error}");
    }

    let cmdline = build_windows_command_line(exe, args);
    let mut cmd_wide = wide_null(&cmdline);
    let current_dir = exe
        .parent()
        .map(|path| path.to_path_buf())
        .unwrap_or_else(|| std::path::PathBuf::from(r"C:\Program Files\Talos\Worker"));
    let current_dir_str = current_dir.to_string_lossy().to_string();
    let current_dir_wide = wide_null(&current_dir_str);
    let (mut environment_block, webview_user_data_dir) = build_chat_environment(primary_token);

    let desktop_wide = wide_null("winsta0\\default");

    let mut si: STARTUPINFOW = unsafe { std::mem::zeroed() };
    si.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
    si.lpDesktop = desktop_wide.as_ptr() as *mut u16;

    let mut pi: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
    let ok = unsafe {
        CreateProcessAsUserW(
            primary_token,
            null(),
            cmd_wide.as_mut_ptr(),
            null_mut(),
            null_mut(),
            FALSE,
            CREATE_UNICODE_ENVIRONMENT,
            environment_block.as_mut_ptr() as *mut c_void,
            current_dir_wide.as_ptr(),
            &mut si,
            &mut pi,
        )
    };
    unsafe { CloseHandle(primary_token) };
    if ok == 0 {
        let os_error = std::io::Error::last_os_error();
        warn!(
            target: CHAT_LOG_TARGET,
            target_session_id = launch_session_id,
            requested_session_id = target_session_id,
            exe = %exe.display(),
            port,
            current_dir = %current_dir_str,
            webview_user_data_dir = ?webview_user_data_dir,
            error = %os_error,
            mode,
            "CreateProcessAsUserW failed for talos_worker_chat"
        );
        anyhow::bail!("CreateProcessAsUserW failed for talos_worker_chat: {os_error}");
    }
    unsafe {
        let _ = CloseHandle(pi.hThread);
        let _ = CloseHandle(pi.hProcess);
    }
    info!(
        target: CHAT_LOG_TARGET,
        target_session_id = launch_session_id,
        requested_session_id = target_session_id,
        port,
        pid = pi.dwProcessId,
        current_dir = %current_dir_str,
        webview_user_data_dir = ?webview_user_data_dir,
        mode,
        "launched talos_worker_chat in user session"
    );
    Ok(())
}

#[cfg(target_os = "macos")]
const MACOS_WORKER_CHAT_APP_PATH: &str = "/Library/Talos/Worker/Talos Worker Chat.app";

#[cfg(target_os = "macos")]
fn command_stdout_trim(program: &str, args: &[&str]) -> Result<String> {
    let output = std::process::Command::new(program).args(args).output()?;
    if !output.status.success() {
        anyhow::bail!("{program} {} failed with {}", args.join(" "), output.status);
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

#[cfg(target_os = "macos")]
fn active_console_user() -> Result<(u32, String)> {
    let uid = command_stdout_trim("/usr/bin/stat", &["-f", "%u", "/dev/console"])?
        .parse::<u32>()
        .context("parse active console uid")?;
    let user = command_stdout_trim("/usr/bin/stat", &["-f", "%Su", "/dev/console"])?;
    if uid == 0 || user.is_empty() || user == "loginwindow" {
        return Err(NoInteractiveUserError.into());
    }
    Ok((uid, user))
}

/// Spawn the chat UI in the active macOS Aqua console session.
#[cfg(target_os = "macos")]
pub fn launch_chat_ui(
    _target_session_id: u32,
    exe: &Path,
    port: u16,
    bridge_secret: &str,
) -> Result<()> {
    launch_chat_ui_with_args(
        exe,
        &build_chat_ui_args(port, bridge_secret),
        port,
        "remote_chat",
    )
}

#[cfg(target_os = "macos")]
pub fn launch_update_reboot_notice_ui(
    _target_session_id: u32,
    exe: &Path,
    port: u16,
    bridge_secret: &str,
    config: &RebootNoticeLaunchConfig,
) -> Result<()> {
    launch_chat_ui_with_args(
        exe,
        &build_update_reboot_notice_args(port, bridge_secret, config),
        port,
        "update_reboot_notice",
    )
}

#[cfg(target_os = "macos")]
pub fn launch_ai_approval_ui(
    _target_session_id: u32,
    exe: &Path,
    port: u16,
    bridge_secret: &str,
    config: &AiApprovalLaunchConfig,
) -> Result<()> {
    launch_chat_ui_with_args(
        exe,
        &build_ai_approval_args(port, bridge_secret, config),
        port,
        "ai_approval",
    )
}

#[cfg(target_os = "macos")]
fn launch_chat_ui_with_args(exe: &Path, args: &[String], port: u16, mode: &str) -> Result<()> {
    const CHAT_LOG_TARGET: &str = "rmm_chat";

    let (uid, user) = active_console_user()?;
    let app_path = Path::new(MACOS_WORKER_CHAT_APP_PATH);

    let mut command = std::process::Command::new("/bin/launchctl");
    command.arg("asuser").arg(uid.to_string());
    if !app_path.exists() {
        anyhow::bail!("Talos Worker Chat app not found: {}", app_path.display());
    }
    command
        .arg("/usr/bin/open")
        .arg("-na")
        .arg(app_path)
        .arg("--args")
        .args(args);

    debug!(
        target: CHAT_LOG_TARGET,
        uid,
        user = %user,
        app_path = %app_path.display(),
        fallback_exe = %exe.display(),
        port,
        uses_app_bundle = true,
        mode,
        "launch_chat_ui requested on macOS"
    );

    let status = command
        .status()
        .context("spawn macOS chat UI via launchctl")?;
    if !status.success() {
        warn!(
            target: CHAT_LOG_TARGET,
            uid,
            user = %user,
            status = %status,
            app_path = %app_path.display(),
            fallback_exe = %exe.display(),
            port,
            mode,
            "launchctl asuser failed for talos_worker_chat"
        );
        anyhow::bail!("launchctl asuser failed for talos_worker_chat: {status}");
    }

    info!(
        target: CHAT_LOG_TARGET,
        uid,
        user = %user,
        app_path = %app_path.display(),
        fallback_exe = %exe.display(),
        port,
        uses_app_bundle = true,
        mode,
        "launched talos_worker_chat in macOS user session"
    );
    Ok(())
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub fn launch_chat_ui(
    _target_session_id: u32,
    _exe: &std::path::Path,
    _port: u16,
    _bridge_secret: &str,
) -> anyhow::Result<()> {
    anyhow::bail!("chat UI launch is only supported on Windows and macOS")
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub fn launch_update_reboot_notice_ui(
    _target_session_id: u32,
    _exe: &std::path::Path,
    _port: u16,
    _bridge_secret: &str,
    _config: &RebootNoticeLaunchConfig,
) -> anyhow::Result<()> {
    anyhow::bail!("chat UI launch is only supported on Windows and macOS")
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub fn launch_ai_approval_ui(
    _target_session_id: u32,
    _exe: &std::path::Path,
    _port: u16,
    _bridge_secret: &str,
    _config: &AiApprovalLaunchConfig,
) -> anyhow::Result<()> {
    anyhow::bail!("chat UI launch is only supported on Windows and macOS")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn update_reboot_notice_args_include_mode_and_metadata() {
        let config = RebootNoticeLaunchConfig {
            notice_id: "notice-1".to_string(),
            deadline_unix_ms: 12345,
            deferrals_used: 2,
            max_deferrals: 4,
            delay_minutes: 15,
        };

        let args = build_update_reboot_notice_args(49152, "secret", &config);

        assert_eq!(args[0], "--mode=update-reboot");
        assert!(args.contains(&"--local-port=49152".to_string()));
        assert!(args.contains(&"--bridge-secret=secret".to_string()));
        assert!(args.contains(&"--notice-id=notice-1".to_string()));
        assert!(args.contains(&"--deadline-unix-ms=12345".to_string()));
        assert!(args.contains(&"--deferrals-used=2".to_string()));
        assert!(args.contains(&"--max-deferrals=4".to_string()));
        assert!(args.contains(&"--delay-minutes=15".to_string()));
    }

    #[test]
    fn ai_approval_args_include_mode_and_metadata() {
        let config = AiApprovalLaunchConfig {
            approval_id: "approval-1".to_string(),
            requester_label: "Operator One".to_string(),
            requester_email: Some("operator@example.com".to_string()),
            organization_name: Some("Example Org".to_string()),
            device_label: "dc-01".to_string(),
            reason: "Capture the current screen".to_string(),
            expires_at_unix_ms: 12345,
            approval_window_expires_at_unix_ms: 67890,
        };

        let args = build_ai_approval_args(49152, "secret", &config);

        assert_eq!(args[0], "--mode=ai-approval");
        assert!(args.contains(&"--local-port=49152".to_string()));
        assert!(args.contains(&"--bridge-secret=secret".to_string()));
        assert!(args.contains(&"--approval-id=approval-1".to_string()));
        assert!(args.contains(&"--requester-label=Operator One".to_string()));
        assert!(args.contains(&"--requester-email=operator@example.com".to_string()));
        assert!(args.contains(&"--organization-name=Example Org".to_string()));
        assert!(args.contains(&"--device-label=dc-01".to_string()));
        assert!(args.contains(&"--reason=Capture the current screen".to_string()));
        assert!(args.contains(&"--expires-at-unix-ms=12345".to_string()));
        assert!(args.contains(&"--approval-window-expires-at-unix-ms=67890".to_string()));
    }

    #[test]
    fn no_interactive_user_error_is_classified() {
        use anyhow::Context as _;

        let error = std::result::Result::<(), NoInteractiveUserError>::Err(NoInteractiveUserError)
            .context("launch chat UI for AI approval")
            .unwrap_err();

        assert!(is_no_interactive_user_error(&error));
    }
}
