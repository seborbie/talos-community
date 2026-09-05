//! Interactive shell session backed by Windows ConPTY or a Unix PTY.
//!
//! Lifecycle:
//! 1. [`ShellSession::start`] spawns a PTY-backed process and binds a TCP listener.
//! 2. The caller sends a `shell_offer` with the listener port back to the server.
//! 3. [`ShellSession::run`] accepts **one** viewer connection, authenticates it,
//!    and bridges PTY I/O ↔ TCP using the shell framing protocol defined in
//!    [`talos_protocol`].
//! 4. The session tears down when the process exits, the viewer disconnects,
//!    or an unrecoverable error occurs.

#[cfg(target_family = "unix")]
mod pty;
#[cfg(target_family = "unix")]
use pty::open_unix_pty;

#[cfg(target_family = "unix")]
use std::ffi::CStr;
#[cfg(target_family = "unix")]
use std::ffi::CString;
#[cfg(target_family = "unix")]
use std::fs::File;
use std::io::{Read, Write};
#[cfg(target_family = "unix")]
use std::os::fd::AsRawFd;
#[cfg(target_family = "unix")]
use std::os::unix::process::CommandExt;
#[cfg(target_family = "unix")]
use std::path::Path;
#[cfg(target_family = "unix")]
use std::process::Child;
#[cfg(target_family = "unix")]
use std::process::Command;
#[cfg(target_family = "unix")]
use std::process::Stdio;
use std::sync::Arc;
#[cfg(target_os = "windows")]
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use base64::engine::general_purpose::{
    STANDARD as BASE64_STANDARD, URL_SAFE_NO_PAD as BASE64_URL_SAFE_NO_PAD,
};
use base64::Engine as _;
use chacha20poly1305::ChaCha20Poly1305;
use rustls::pki_types::ServerName;
use talos_protocol::relay_transport::{
    build_e2e_cipher, build_relay_client_tls_config, parse_relay_target, read_e2e_frame_from,
    read_http_response, write_e2e_frame_flush,
};
use talos_protocol::{
    build_shell_exit_payload, build_shell_frame, parse_shell_resize_payload, ShellRunAs,
    SHELL_MSG_AUTH, SHELL_MSG_ERROR, SHELL_MSG_EXIT, SHELL_MSG_INPUT, SHELL_MSG_OUTPUT,
    SHELL_MSG_RESIZE,
};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio::time::interval;
use tokio_rustls::TlsConnector;
use tracing::{debug, error, warn};
#[cfg(target_os = "windows")]
use winapi::shared::minwindef::{DWORD, FALSE, LPVOID};
#[cfg(target_os = "windows")]
use winapi::um::errhandlingapi::GetLastError;
#[cfg(target_os = "windows")]
use winapi::um::handleapi::CloseHandle;
#[cfg(target_os = "windows")]
use winapi::um::libloaderapi::{GetModuleHandleA, GetProcAddress, LoadLibraryA};
#[cfg(target_os = "windows")]
use winapi::um::namedpipeapi::CreatePipe;
#[cfg(target_os = "windows")]
use winapi::um::processthreadsapi::{
    CreateProcessAsUserW, CreateProcessW, DeleteProcThreadAttributeList, GetExitCodeProcess,
    GetProcessId, InitializeProcThreadAttributeList, TerminateProcess, UpdateProcThreadAttribute,
    LPPROC_THREAD_ATTRIBUTE_LIST, PROCESS_INFORMATION,
};
#[cfg(target_os = "windows")]
use winapi::um::securitybaseapi::DuplicateTokenEx as DuplicateTokenExSecurity;
#[cfg(target_os = "windows")]
use winapi::um::synchapi::WaitForSingleObject;
#[cfg(target_os = "windows")]
use winapi::um::winbase::{
    WTSGetActiveConsoleSessionId, CREATE_UNICODE_ENVIRONMENT, EXTENDED_STARTUPINFO_PRESENT,
    INFINITE, STARTF_USESTDHANDLES, STARTUPINFOEXW, WAIT_OBJECT_0,
};
#[cfg(target_os = "windows")]
use winapi::um::wincontypes::{COORD, HPCON};
#[cfg(target_os = "windows")]
use winapi::um::winnt::{SecurityImpersonation, TokenPrimary, HANDLE, MAXIMUM_ALLOWED};
#[cfg(target_os = "windows")]
use winapi::um::wtsapi32::WTSQueryUserToken;

/// Default terminal dimensions when none are provided by the viewer.
const DEFAULT_COLS: i16 = 120;
const DEFAULT_ROWS: i16 = 30;

/// Read buffer size for PTY stdout (8 KiB - good balance between latency
/// and syscall overhead for terminal output).
const PTY_READ_BUF: usize = 8192;

/// Channel capacity for ConPTY output chunks waiting to be written to TCP.
const OUTPUT_CHANNEL_CAP: usize = 128;

/// Channel capacity for viewer input chunks waiting to be written to ConPTY.
const INPUT_CHANNEL_CAP: usize = 64;

/// Maximum time to wait for the viewer to connect and authenticate (seconds).
const AUTH_TIMEOUT_SECS: u64 = 30;

/// Relay heartbeat interval (seconds).
const RELAY_HEARTBEAT_INTERVAL_SECS: u64 = 15;

/// Relay heartbeat payload (kept identical to desktop transport for consistency).
const RELAY_HEARTBEAT_PAYLOAD: &[u8] = b"heartbeat";
#[cfg(target_os = "windows")]
const WAIT_TIMEOUT_DWORD: DWORD = 0x0000_0102;
#[cfg(target_os = "windows")]
const PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE: usize = 0x0002_0016;

pub enum ShellProcess {
    #[cfg(target_family = "unix")]
    Unix(UnixShellProcess),
    #[cfg(target_os = "windows")]
    User(UserShellProcess),
}

impl ShellProcess {
    fn output(&mut self) -> Result<Box<dyn Read + Send>> {
        match self {
            #[cfg(target_family = "unix")]
            ShellProcess::Unix(proc) => Ok(Box::new(proc.output()?)),
            #[cfg(target_os = "windows")]
            ShellProcess::User(proc) => Ok(Box::new(proc.output()?)),
        }
    }

    fn input(&mut self) -> Result<Box<dyn Write + Send>> {
        match self {
            #[cfg(target_family = "unix")]
            ShellProcess::Unix(proc) => Ok(Box::new(proc.input()?)),
            #[cfg(target_os = "windows")]
            ShellProcess::User(proc) => Ok(Box::new(proc.input()?)),
        }
    }

    fn resize(&mut self, cols: i16, rows: i16) -> Result<()> {
        match self {
            #[cfg(target_family = "unix")]
            ShellProcess::Unix(proc) => proc.resize(cols, rows),
            #[cfg(target_os = "windows")]
            ShellProcess::User(proc) => proc.resize(cols, rows),
        }
    }

    fn is_alive(&mut self) -> bool {
        match self {
            #[cfg(target_family = "unix")]
            ShellProcess::Unix(proc) => proc.is_alive(),
            #[cfg(target_os = "windows")]
            ShellProcess::User(proc) => proc.is_alive(),
        }
    }

    fn exit(&mut self, code: u32) -> Result<()> {
        match self {
            #[cfg(target_family = "unix")]
            ShellProcess::Unix(proc) => proc.exit(code),
            #[cfg(target_os = "windows")]
            ShellProcess::User(proc) => proc.exit(code),
        }
    }

    fn wait(&mut self, timeout_millis: Option<u32>) -> Result<u32> {
        match self {
            #[cfg(target_family = "unix")]
            ShellProcess::Unix(proc) => proc.wait(timeout_millis),
            #[cfg(target_os = "windows")]
            ShellProcess::User(proc) => proc.wait(timeout_millis),
        }
    }
}

#[cfg(target_family = "unix")]
pub struct UnixShellProcess {
    master: File,
    child: Child,
}

#[cfg(target_family = "unix")]
impl UnixShellProcess {
    fn output(&self) -> Result<File> {
        self.master.try_clone().context("clone Unix PTY master")
    }

    fn input(&self) -> Result<File> {
        self.master.try_clone().context("clone Unix PTY master")
    }

    fn resize(&mut self, cols: i16, rows: i16) -> Result<()> {
        let winsize = libc::winsize {
            ws_row: positive_dimension(rows, DEFAULT_ROWS) as libc::c_ushort,
            ws_col: positive_dimension(cols, DEFAULT_COLS) as libc::c_ushort,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        let result = unsafe { libc::ioctl(self.master.as_raw_fd(), libc::TIOCSWINSZ, &winsize) };
        if result != 0 {
            return Err(std::io::Error::last_os_error()).context("resize Unix PTY");
        }
        Ok(())
    }

    fn is_alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    fn exit(&mut self, _code: u32) -> Result<()> {
        match self.child.try_wait().context("poll Unix shell process")? {
            Some(_) => Ok(()),
            None => self.child.kill().context("kill Unix shell process"),
        }
    }

    fn wait(&mut self, timeout_millis: Option<u32>) -> Result<u32> {
        let status = if timeout_millis == Some(0) {
            self.child
                .try_wait()
                .context("poll Unix shell process")?
                .ok_or_else(|| anyhow::anyhow!("wait timeout"))?
        } else {
            self.child.wait().context("wait Unix shell process")?
        };
        Ok(status.code().unwrap_or(1) as u32)
    }
}

#[cfg(target_family = "unix")]
fn positive_dimension(value: i16, fallback: i16) -> u16 {
    if value > 0 {
        value as u16
    } else {
        fallback as u16
    }
}

#[cfg(target_family = "unix")]
impl Drop for UnixShellProcess {
    fn drop(&mut self) {
        if self.is_alive() {
            let _ = self.child.kill();
        }
    }
}

#[cfg(target_os = "windows")]
pub struct UserShellProcess {
    stdin_write: std::fs::File,
    stdout_read: std::fs::File,
    process_handle: HANDLE,
    thread_handle: HANDLE,
    console_handle: HPCON,
}

#[cfg(target_os = "windows")]
unsafe impl Send for UserShellProcess {}

#[cfg(target_os = "windows")]
unsafe impl Sync for UserShellProcess {}

#[cfg(target_os = "windows")]
#[derive(Clone, Copy)]
struct ConptyApi {
    create_pseudo_console:
        unsafe extern "system" fn(COORD, HANDLE, HANDLE, DWORD, *mut HPCON) -> i32,
    resize_pseudo_console: unsafe extern "system" fn(HPCON, COORD) -> i32,
    close_pseudo_console: unsafe extern "system" fn(HPCON),
}

#[cfg(target_os = "windows")]
static CONPTY_API: OnceLock<Option<ConptyApi>> = OnceLock::new();

#[cfg(target_os = "windows")]
fn conpty_api() -> Option<&'static ConptyApi> {
    CONPTY_API
        .get_or_init(|| unsafe {
            let kernel32_name = b"kernel32.dll\0";
            let module = {
                let loaded = GetModuleHandleA(kernel32_name.as_ptr() as *const i8);
                if loaded.is_null() {
                    LoadLibraryA(kernel32_name.as_ptr() as *const i8)
                } else {
                    loaded
                }
            };
            if module.is_null() {
                return None;
            }

            let create_pseudo_console = load_kernel32_proc(module, b"CreatePseudoConsole\0")?;
            let resize_pseudo_console = load_kernel32_proc(module, b"ResizePseudoConsole\0")?;
            let close_pseudo_console = load_kernel32_proc(module, b"ClosePseudoConsole\0")?;
            Some(ConptyApi {
                create_pseudo_console,
                resize_pseudo_console,
                close_pseudo_console,
            })
        })
        .as_ref()
}

#[cfg(target_os = "windows")]
unsafe fn load_kernel32_proc<T: Copy>(
    module: winapi::shared::minwindef::HMODULE,
    name: &'static [u8],
) -> Option<T> {
    let proc = GetProcAddress(module, name.as_ptr() as *const i8);
    if proc.is_null() {
        return None;
    }
    Some(std::mem::transmute_copy::<_, T>(&proc))
}

#[cfg(target_os = "windows")]
fn conpty_api_or_err() -> Result<&'static ConptyApi> {
    conpty_api().ok_or_else(|| {
        anyhow::anyhow!(
            "Windows ConPTY is not available; interactive shell requires Windows Server 2019, Windows 10 1809, or newer"
        )
    })
}

#[cfg(target_os = "windows")]
impl UserShellProcess {
    fn pid(&self) -> u32 {
        unsafe { GetProcessId(self.process_handle) }
    }

    fn output(&self) -> Result<std::fs::File> {
        self.stdout_read
            .try_clone()
            .context("clone user shell stdout")
    }

    fn input(&self) -> Result<std::fs::File> {
        self.stdin_write
            .try_clone()
            .context("clone user shell stdin")
    }

    fn resize(&mut self, cols: i16, rows: i16) -> Result<()> {
        let api = conpty_api_or_err()?;
        let hr =
            unsafe { (api.resize_pseudo_console)(self.console_handle, COORD { X: cols, Y: rows }) };
        if hr < 0 {
            return Err(anyhow::anyhow!("ResizePseudoConsole failed: {hr}"));
        }
        Ok(())
    }

    fn is_alive(&self) -> bool {
        unsafe { WaitForSingleObject(self.process_handle, 0) == WAIT_TIMEOUT_DWORD }
    }

    fn exit(&mut self, code: u32) -> Result<()> {
        let ok = unsafe { TerminateProcess(self.process_handle, code) };
        if ok == 0 {
            let err = unsafe { GetLastError() };
            return Err(anyhow::anyhow!("TerminateProcess failed: {err}"));
        }
        Ok(())
    }

    fn wait(&self, timeout_millis: Option<u32>) -> Result<u32> {
        let wait_result =
            unsafe { WaitForSingleObject(self.process_handle, timeout_millis.unwrap_or(INFINITE)) };
        if wait_result == WAIT_TIMEOUT_DWORD {
            return Err(anyhow::anyhow!("wait timeout"));
        }
        if wait_result != WAIT_OBJECT_0 {
            return Err(anyhow::anyhow!("wait failed: {wait_result}"));
        }
        let mut exit_code: DWORD = 0;
        let ok = unsafe { GetExitCodeProcess(self.process_handle, &mut exit_code) };
        if ok == 0 {
            let err = unsafe { GetLastError() };
            return Err(anyhow::anyhow!("GetExitCodeProcess failed: {err}"));
        }
        Ok(exit_code)
    }
}

#[cfg(target_os = "windows")]
impl Drop for UserShellProcess {
    fn drop(&mut self) {
        unsafe {
            if !self.console_handle.is_null() {
                if let Some(api) = conpty_api() {
                    (api.close_pseudo_console)(self.console_handle);
                }
            }
            if !self.process_handle.is_null() {
                let _ = CloseHandle(self.process_handle);
            }
            if !self.thread_handle.is_null() {
                let _ = CloseHandle(self.thread_handle);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// ShellSession
// ---------------------------------------------------------------------------

/// An interactive shell session backed by a platform PTY.
pub struct ShellSession {
    session_id: String,
    token: String,
    process: ShellProcess,
    listener: TcpListener,
}

impl ShellSession {
    /// Spawn a PTY shell process and bind a TCP listener for the viewer.
    ///
    /// Returns the session and the TCP port the viewer should connect to.
    pub async fn start(
        session_id: String,
        token: String,
        run_as: ShellRunAs,
        target_session_id: Option<u32>,
    ) -> Result<(Self, u16)> {
        let process = spawn_shell_process(session_id.clone(), run_as, target_session_id).await?;

        // Bind a TCP listener on an ephemeral port (0 = OS-assigned).
        let listener = TcpListener::bind("0.0.0.0:0")
            .await
            .context("bind shell TCP listener")?;
        let port = listener
            .local_addr()
            .context("get listener local addr")?
            .port();

        debug!(
            session_id = %session_id,
            port = port,
            "shell TCP listener bound"
        );

        Ok((
            Self {
                session_id,
                token,
                process,
                listener,
            },
            port,
        ))
    }

    /// Accept a single viewer connection, authenticate, and bridge I/O until
    /// the process exits or the viewer disconnects.
    ///
    /// This consumes the session — it is a one-shot lifetime. Call from a
    /// spawned tokio task.
    pub async fn run(mut self) {
        let sid = self.session_id.clone();

        // ── 1. Accept one TCP connection with timeout ──────────────────────
        let tcp_stream = match tokio::time::timeout(
            std::time::Duration::from_secs(AUTH_TIMEOUT_SECS),
            self.listener.accept(),
        )
        .await
        {
            Ok(Ok((stream, addr))) => {
                debug!(session_id = %sid, peer = %addr, "viewer connected to shell");
                stream
            }
            Ok(Err(e)) => {
                error!(session_id = %sid, error = %e, "TCP accept failed");
                return;
            }
            Err(_) => {
                warn!(session_id = %sid, "no viewer connected within timeout");
                return;
            }
        };

        // Drop the listener — we only allow one connection per session.
        drop(self.listener);

        let (mut tcp_read, mut tcp_write) = tcp_stream.into_split();

        // ── 2. Authenticate: first frame must be SHELL_MSG_AUTH ────────────
        match read_shell_frame(&mut tcp_read).await {
            Ok(frame) if frame.message_type == SHELL_MSG_AUTH => {
                let received_token = String::from_utf8_lossy(&frame.payload);
                if received_token != self.token {
                    warn!(session_id = %sid, "shell auth token mismatch");
                    let _ = write_shell_frame(
                        &mut tcp_write,
                        SHELL_MSG_ERROR,
                        b"authentication failed",
                    )
                    .await;
                    return;
                }
                debug!(session_id = %sid, "shell auth succeeded");
            }
            Ok(frame) => {
                warn!(
                    session_id = %sid,
                    msg_type = frame.message_type,
                    "expected auth frame, got different type"
                );
                return;
            }
            Err(e) => {
                warn!(session_id = %sid, error = %e, "failed to read auth frame");
                return;
            }
        }

        // ── 3. Obtain PTY pipe handles ────────────────────────────────────
        let pty_reader = match self.process.output() {
            Ok(r) => r,
            Err(e) => {
                error!(session_id = %sid, error = %e, "failed to get PTY output pipe");
                return;
            }
        };
        let pty_writer = match self.process.input() {
            Ok(w) => w,
            Err(e) => {
                error!(session_id = %sid, error = %e, "failed to get PTY input pipe");
                return;
            }
        };

        // ── 4. Spawn blocking task: PTY stdout -> output channel ──────────
        let (output_tx, mut output_rx) = mpsc::channel::<Vec<u8>>(OUTPUT_CHANNEL_CAP);
        let sid_out = sid.clone();

        let output_handle = tokio::task::spawn_blocking(move || {
            let mut reader = pty_reader;
            let mut buf = [0u8; PTY_READ_BUF];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => {
                        debug!(session_id = %sid_out, "PTY stdout EOF");
                        break;
                    }
                    Ok(n) => {
                        if output_tx.blocking_send(buf[..n].to_vec()).is_err() {
                            // Receiver dropped — session is tearing down.
                            break;
                        }
                    }
                    Err(e) => {
                        debug!(session_id = %sid_out, error = %e, "PTY stdout read error");
                        break;
                    }
                }
            }
        });

        // ── 5. Spawn blocking task: input channel -> PTY stdin ────────────
        let (input_tx, mut input_rx) = mpsc::channel::<Vec<u8>>(INPUT_CHANNEL_CAP);
        let sid_in = sid.clone();

        let input_handle = tokio::task::spawn_blocking(move || {
            let mut writer = pty_writer;
            while let Some(data) = input_rx.blocking_recv() {
                if let Err(e) = writer.write_all(&data) {
                    debug!(session_id = %sid_in, error = %e, "PTY stdin write error");
                    break;
                }
            }
        });

        // ── 6. Main async bridge: TCP ↔ channels + resize ─────────────────
        let sid_bridge = sid.clone();
        let bridge_result: Result<(), anyhow::Error> = async {
            loop {
                tokio::select! {
                    // ConPTY output → TCP
                    output = output_rx.recv() => {
                        match output {
                            Some(data) => {
                                write_shell_frame(&mut tcp_write, SHELL_MSG_OUTPUT, &data).await
                                    .context("write output frame")?;
                            }
                            None => {
                                // ConPTY output closed — process likely exited.
                                debug!(session_id = %sid_bridge, "output channel closed");
                                break;
                            }
                        }
                    }
                    // TCP → parse frames → dispatch
                    frame_result = read_shell_frame(&mut tcp_read) => {
                        match frame_result {
                            Ok(frame) => {
                                match frame.message_type {
                                    SHELL_MSG_INPUT => {
                                        if input_tx.send(frame.payload).await.is_err() {
                                            debug!(session_id = %sid_bridge, "input channel closed");
                                            break;
                                        }
                                    }
                                    SHELL_MSG_RESIZE => {
                                        if let Some((cols, rows)) = parse_shell_resize_payload(&frame.payload) {
                                            // resize() is a quick platform PTY call, OK in async context.
                                            if let Err(e) = self.process.resize(cols as i16, rows as i16) {
                                                warn!(
                                                    session_id = %sid_bridge,
                                                    error = %e,
                                                    "PTY resize failed"
                                                );
                                            } else {
                                                debug!(
                                                    session_id = %sid_bridge,
                                                    cols, rows,
                                                    "terminal resized"
                                                );
                                            }
                                        }
                                    }
                                    other => {
                                        warn!(
                                            session_id = %sid_bridge,
                                            msg_type = other,
                                            "unexpected shell frame type"
                                        );
                                    }
                                }
                            }
                            Err(e) => {
                                debug!(session_id = %sid_bridge, error = %e, "TCP read ended");
                                break;
                            }
                        }
                    }
                }
            }
            Ok(())
        }
        .await;

        if let Err(e) = &bridge_result {
            warn!(session_id = %sid, error = %e, "shell bridge error");
        }

        // ── 7. Cleanup: send exit frame, terminate process ─────────────────
        // Try to get exit code and notify viewer.
        let exit_code = if self.process.is_alive() {
            let _ = self.process.exit(0);
            0u32
        } else {
            self.process.wait(Some(0)).unwrap_or(1)
        };

        let exit_payload = build_shell_exit_payload(exit_code);
        let _ = write_shell_frame(&mut tcp_write, SHELL_MSG_EXIT, &exit_payload).await;

        // Drop channels to unblock blocking tasks.
        drop(input_tx);
        drop(output_rx);

        // Wait for blocking tasks to finish (bounded — they will exit once
        // pipes close or channels drop).
        let _ = tokio::time::timeout(std::time::Duration::from_secs(5), output_handle).await;
        let _ = tokio::time::timeout(std::time::Duration::from_secs(5), input_handle).await;

        if exit_code != 0 {
            warn!(session_id = %sid, exit_code, "shell session ended with non-zero exit");
        } else {
            debug!(session_id = %sid, exit_code, "shell session ended");
        }
    }
}

async fn spawn_shell_process(
    session_id: String,
    run_as: ShellRunAs,
    target_session_id: Option<u32>,
) -> Result<ShellProcess> {
    let sid = session_id.clone();
    tokio::task::spawn_blocking(move || -> Result<ShellProcess> {
        #[cfg(not(target_os = "windows"))]
        let _ = target_session_id;
        match run_as {
            ShellRunAs::System => spawn_system_shell_process(&sid),
            ShellRunAs::User => {
                #[cfg(target_os = "windows")]
                {
                    let proc = spawn_user_shell_process(&sid, target_session_id)?;
                    debug!(session_id = %sid, pid = proc.pid(), "user-token shell process spawned");
                    Ok(ShellProcess::User(proc))
                }
                #[cfg(target_os = "macos")]
                {
                    spawn_macos_console_user_shell_process(&sid)
                }
                #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
                {
                    Err(anyhow::anyhow!("run_as=user is only supported on Windows"))
                }
            }
        }
    })
    .await
    .context("spawn_blocking panicked")?
}

#[cfg(target_os = "windows")]
fn spawn_system_shell_process(session_id: &str) -> Result<ShellProcess> {
    let proc = spawn_windows_conpty_shell_process("powershell.exe -NoLogo -NoProfile", None)
        .context("spawn system ConPTY shell process")?;
    debug!(
        session_id = %session_id,
        pid = proc.pid(),
        "system ConPTY process spawned"
    );
    Ok(ShellProcess::User(proc))
}

#[cfg(target_family = "unix")]
struct UnixShellUser {
    username: String,
    uid: libc::uid_t,
    gid: libc::gid_t,
    home: String,
    shell: String,
}

#[cfg(target_family = "unix")]
#[cfg(all(target_family = "unix", not(target_os = "macos")))]
fn shell_root_allowed() -> bool {
    std::env::var("RMM_SHELL_ALLOW_ROOT")
        .ok()
        .map(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(false)
}

#[cfg(target_family = "unix")]
fn passwd_field_to_string(ptr: *const libc::c_char, field: &str) -> Result<String> {
    if ptr.is_null() {
        return Err(anyhow::anyhow!("shell user passwd field {field} is null"));
    }
    Ok(unsafe { CStr::from_ptr(ptr) }
        .to_string_lossy()
        .into_owned())
}

#[cfg(all(target_family = "unix", not(target_os = "macos")))]
fn resolve_unix_shell_user() -> Result<UnixShellUser> {
    let username = crate::linux_account::resolve_managed_shell_username()
        .or_else(|| {
            std::env::var("RMM_SHELL_USER")
                .ok()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
        })
        .unwrap_or_else(|| "talos".to_string());
    let username_c = CString::new(username.clone()).context("RMM_SHELL_USER contains NUL")?;

    let mut passwd: libc::passwd = unsafe { std::mem::zeroed() };
    let mut result: *mut libc::passwd = std::ptr::null_mut();
    let mut buffer = vec![0u8; 16 * 1024];
    let rc = unsafe {
        libc::getpwnam_r(
            username_c.as_ptr(),
            &mut passwd,
            buffer.as_mut_ptr() as *mut libc::c_char,
            buffer.len(),
            &mut result,
        )
    };
    if rc != 0 {
        return Err(std::io::Error::from_raw_os_error(rc))
            .with_context(|| format!("resolve RMM shell user {username}"));
    }
    if result.is_null() {
        return Err(anyhow::anyhow!(
            "RMM shell user '{username}' does not exist; create it or set RMM_SHELL_USER"
        ));
    }

    if passwd.pw_uid == 0 && !shell_root_allowed() {
        return Err(anyhow::anyhow!(
            "RMM shell user '{username}' resolves to root; set RMM_SHELL_ALLOW_ROOT=1 to override"
        ));
    }

    let home = passwd_field_to_string(passwd.pw_dir, "home")?;
    let shell = passwd_field_to_string(passwd.pw_shell, "shell")?;
    let home = if home.trim().is_empty() {
        "/".to_string()
    } else {
        home
    };
    let shell = if shell.trim().is_empty() {
        "/bin/sh".to_string()
    } else {
        shell
    };
    let shell_name = Path::new(&shell)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    if matches!(shell_name, "nologin" | "false") {
        return Err(anyhow::anyhow!(
            "RMM shell user '{username}' has non-interactive shell {shell}"
        ));
    }
    if !Path::new(&shell).exists() {
        return Err(anyhow::anyhow!(
            "RMM shell user '{username}' shell {shell} does not exist"
        ));
    }

    Ok(UnixShellUser {
        username,
        uid: passwd.pw_uid,
        gid: passwd.pw_gid,
        home,
        shell,
    })
}

#[cfg(target_os = "macos")]
fn resolve_unix_shell_user() -> Result<UnixShellUser> {
    if unsafe { libc::geteuid() } != 0 {
        return Err(anyhow::anyhow!(
            "macOS system shell requires the Talos worker to run as root via LaunchDaemon"
        ));
    }
    let shell = if Path::new("/bin/zsh").exists() {
        "/bin/zsh"
    } else {
        "/bin/sh"
    };
    Ok(UnixShellUser {
        username: "root".to_string(),
        uid: 0,
        gid: 0,
        home: "/var/root".to_string(),
        shell: shell.to_string(),
    })
}

#[cfg(target_os = "macos")]
fn fallback_macos_shell() -> String {
    if Path::new("/bin/zsh").exists() {
        "/bin/zsh".to_string()
    } else {
        "/bin/sh".to_string()
    }
}

#[cfg(target_os = "macos")]
fn macos_effective_home(username: &str, home: String) -> String {
    let home = home.trim();
    if !home.is_empty() {
        return home.to_string();
    }
    if username.trim().is_empty() {
        "/".to_string()
    } else {
        format!("/Users/{}", username.trim())
    }
}

#[cfg(target_os = "macos")]
fn macos_effective_shell(shell: Option<String>) -> String {
    let shell = shell.unwrap_or_default();
    let shell = shell.trim();
    if shell.is_empty() {
        return fallback_macos_shell();
    }
    let shell_name = Path::new(shell)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    if matches!(shell_name, "nologin" | "false") || !Path::new(shell).exists() {
        return fallback_macos_shell();
    }
    shell.to_string()
}

#[cfg(target_os = "macos")]
fn resolve_macos_console_shell_user() -> Result<UnixShellUser> {
    if unsafe { libc::geteuid() } != 0 {
        return Err(anyhow::anyhow!(
            "macOS user shell requires the Talos worker to run as root via LaunchDaemon"
        ));
    }

    let metadata = std::fs::metadata("/dev/console").context("stat /dev/console")?;
    use std::os::unix::fs::MetadataExt;
    let uid = metadata.uid() as libc::uid_t;
    if uid == 0 {
        return Err(anyhow::anyhow!(
            "no active macOS console user is logged in for run_as=user shell"
        ));
    }

    let mut passwd: libc::passwd = unsafe { std::mem::zeroed() };
    let mut result: *mut libc::passwd = std::ptr::null_mut();
    let mut buffer = vec![0u8; 16 * 1024];
    let rc = unsafe {
        libc::getpwuid_r(
            uid,
            &mut passwd,
            buffer.as_mut_ptr() as *mut libc::c_char,
            buffer.len(),
            &mut result,
        )
    };
    if rc != 0 {
        return Err(std::io::Error::from_raw_os_error(rc))
            .with_context(|| format!("resolve macOS console user uid {uid}"));
    }
    if result.is_null() {
        return Err(anyhow::anyhow!(
            "macOS console user uid {uid} does not exist"
        ));
    }

    let username = passwd_field_to_string(passwd.pw_name, "username")?;
    let home = macos_effective_home(
        &username,
        passwd_field_to_string(passwd.pw_dir, "home").unwrap_or_default(),
    );
    let shell = macos_effective_shell(passwd_field_to_string(passwd.pw_shell, "shell").ok());

    Ok(UnixShellUser {
        username,
        uid: passwd.pw_uid,
        gid: passwd.pw_gid,
        home,
        shell,
    })
}

#[cfg(target_os = "macos")]
fn unix_shell_path() -> &'static str {
    "/opt/homebrew/sbin:/opt/homebrew/bin:/usr/local/sbin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin"
}

#[cfg(all(target_family = "unix", not(target_os = "macos")))]
fn unix_shell_path() -> &'static str {
    "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin"
}

#[cfg(target_family = "unix")]
fn spawn_system_shell_process(session_id: &str) -> Result<ShellProcess> {
    let user = resolve_unix_shell_user()?;
    spawn_unix_shell_process_for_user(session_id, user)
}

#[cfg(target_os = "macos")]
fn spawn_macos_console_user_shell_process(session_id: &str) -> Result<ShellProcess> {
    let user = resolve_macos_console_shell_user()?;
    spawn_unix_shell_process_for_user(session_id, user)
}

#[cfg(target_family = "unix")]
fn spawn_unix_shell_process_for_user(
    session_id: &str,
    user: UnixShellUser,
) -> Result<ShellProcess> {
    let (master, slave) = open_unix_pty()?;
    let slave_fd = slave.as_raw_fd();
    let username_c = CString::new(user.username.clone()).context("shell username contains NUL")?;
    let shell_name = Path::new(&user.shell)
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("sh");
    let arg0 = format!("-{shell_name}");
    let current_dir = if Path::new(&user.home).is_dir() {
        Path::new(&user.home)
    } else {
        Path::new("/")
    };

    let mut cmd = Command::new(&user.shell);
    cmd.arg0(arg0)
        .arg("-i")
        .env_clear()
        .env("PATH", unix_shell_path())
        .env("TERM", "xterm-256color")
        .env("HOME", &user.home)
        .env("USER", &user.username)
        .env("LOGNAME", &user.username)
        .env("SHELL", &user.shell)
        .current_dir(current_dir)
        .stdin(Stdio::from(
            slave.try_clone().context("clone PTY slave stdin")?,
        ))
        .stdout(Stdio::from(
            slave.try_clone().context("clone PTY slave stdout")?,
        ))
        .stderr(Stdio::from(
            slave.try_clone().context("clone PTY slave stderr")?,
        ));

    if let Ok(lang) = std::env::var("LANG") {
        cmd.env("LANG", lang);
    }

    let uid = user.uid;
    let gid = user.gid;
    unsafe {
        cmd.pre_exec(move || {
            if libc::setsid() < 0 {
                return Err(std::io::Error::last_os_error());
            }
            if libc::ioctl(slave_fd, libc::TIOCSCTTY as _, 0) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            #[cfg(target_os = "macos")]
            if libc::initgroups(username_c.as_ptr(), gid as libc::c_int) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            #[cfg(not(target_os = "macos"))]
            if libc::initgroups(username_c.as_ptr(), gid) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            if libc::setgid(gid) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            if libc::setuid(uid) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }

    let child = cmd.spawn().context("spawn Unix PTY shell process")?;
    drop(slave);
    debug!(
        session_id = %session_id,
        pid = child.id(),
        shell = %user.shell,
        user = %user.username,
        uid = user.uid,
        "Unix PTY shell process spawned"
    );

    Ok(ShellProcess::Unix(UnixShellProcess { master, child }))
}

#[cfg(target_os = "windows")]
fn close_handle_if_valid(handle: HANDLE) {
    if !handle.is_null() {
        unsafe {
            let _ = CloseHandle(handle);
        }
    }
}

#[cfg(target_os = "windows")]
fn select_logged_in_session_id(
    shell_session_id: &str,
    preferred_session_id: Option<u32>,
) -> Option<u32> {
    let sessions = crate::display::enumerate_wts_sessions();
    if let Some(preferred_session_id) = preferred_session_id {
        if let Some(session) = sessions
            .iter()
            .find(|session| session.session_id == preferred_session_id)
        {
            if !session.user_name.trim().is_empty() {
                debug!(
                    shell_session_id = %shell_session_id,
                    target_session_id = session.session_id,
                    user_name = %session.user_name,
                    state = %session.state,
                    "selected explicitly requested user session for shell impersonation"
                );
                return Some(session.session_id);
            }
            warn!(
                shell_session_id = %shell_session_id,
                target_session_id = preferred_session_id,
                "requested shell target session has no logged-in user"
            );
            return None;
        }
        warn!(
            shell_session_id = %shell_session_id,
            target_session_id = preferred_session_id,
            "requested shell target session was not found"
        );
        return None;
    }

    let active_console_session_id = unsafe { WTSGetActiveConsoleSessionId() };
    if active_console_session_id != u32::MAX {
        if let Some(session) = sessions.iter().find(|session| {
            session.session_id == active_console_session_id
                && session.state.eq_ignore_ascii_case("active")
                && !session.user_name.trim().is_empty()
        }) {
            debug!(
                shell_session_id = %shell_session_id,
                target_session_id = session.session_id,
                user_name = %session.user_name,
                "selected active console user session for shell impersonation"
            );
            return Some(session.session_id);
        }
    }

    if let Some(session) = sessions.iter().find(|session| {
        session.state.eq_ignore_ascii_case("active") && !session.user_name.trim().is_empty()
    }) {
        debug!(
            shell_session_id = %shell_session_id,
            target_session_id = session.session_id,
            user_name = %session.user_name,
            "selected active non-console user session for shell impersonation"
        );
        return Some(session.session_id);
    }

    warn!(
        shell_session_id = %shell_session_id,
        active_console_session_id = active_console_session_id,
        sessions = ?sessions,
        "no active logged-in user session available for shell impersonation"
    );
    None
}

#[cfg(target_os = "windows")]
fn spawn_user_shell_process(
    session_id: &str,
    preferred_session_id: Option<u32>,
) -> Result<UserShellProcess> {
    use std::ptr::null_mut;

    let target_session_id = select_logged_in_session_id(session_id, preferred_session_id)
        .ok_or_else(|| {
            if let Some(requested) = preferred_session_id {
                anyhow::anyhow!("requested user session {requested} is not available")
            } else {
                anyhow::anyhow!("no active logged-in user session found")
            }
        })?;

    debug!(
        session_id = %session_id,
        target_session_id = target_session_id,
        "starting user-token shell process"
    );

    let mut user_token: HANDLE = null_mut();
    let token_ok = unsafe { WTSQueryUserToken(target_session_id as DWORD, &mut user_token) };
    if token_ok == 0 || user_token.is_null() {
        let err = unsafe { GetLastError() };
        return Err(anyhow::anyhow!(
            "WTSQueryUserToken failed for session {target_session_id}: {err}"
        ));
    }

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
    close_handle_if_valid(user_token);
    if dup_ok == 0 || primary_token.is_null() {
        let err = unsafe { GetLastError() };
        return Err(anyhow::anyhow!("DuplicateTokenEx failed: {err}"));
    }

    let result = spawn_windows_conpty_shell_process(
        "powershell.exe -NoLogo -NoProfile",
        Some(primary_token),
    )
    .context("spawn user-token ConPTY shell process");
    close_handle_if_valid(primary_token);
    result
}

#[cfg(target_os = "windows")]
fn spawn_windows_conpty_shell_process(
    command_line: &str,
    primary_token: Option<HANDLE>,
) -> Result<UserShellProcess> {
    use std::mem::{size_of, zeroed};
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::io::{FromRawHandle, RawHandle};
    use std::ptr::{null, null_mut};

    let api = conpty_api_or_err()?;

    let mut pty_input: HANDLE = null_mut();
    let mut con_writer: HANDLE = null_mut();
    let input_pipe_ok = unsafe { CreatePipe(&mut pty_input, &mut con_writer, null_mut(), 0) };
    if input_pipe_ok == 0 {
        let err = unsafe { GetLastError() };
        return Err(anyhow::anyhow!("CreatePipe(conpty input) failed: {err}"));
    }

    let mut con_reader: HANDLE = null_mut();
    let mut pty_output: HANDLE = null_mut();
    let output_pipe_ok = unsafe { CreatePipe(&mut con_reader, &mut pty_output, null_mut(), 0) };
    if output_pipe_ok == 0 {
        let err = unsafe { GetLastError() };
        close_handle_if_valid(pty_input);
        close_handle_if_valid(con_writer);
        return Err(anyhow::anyhow!("CreatePipe(conpty output) failed: {err}"));
    }

    let mut pseudo_console: HPCON = null_mut();
    let create_console_hr = unsafe {
        (api.create_pseudo_console)(
            COORD {
                X: DEFAULT_COLS,
                Y: DEFAULT_ROWS,
            },
            pty_input,
            pty_output,
            0,
            &mut pseudo_console,
        )
    };
    close_handle_if_valid(pty_input);
    close_handle_if_valid(pty_output);
    if create_console_hr < 0 || pseudo_console.is_null() {
        close_handle_if_valid(con_reader);
        close_handle_if_valid(con_writer);
        return Err(anyhow::anyhow!(
            "CreatePseudoConsole failed: {create_console_hr}"
        ));
    }

    let mut attribute_list_size: usize = 0;
    unsafe {
        let _ = InitializeProcThreadAttributeList(null_mut(), 1, 0, &mut attribute_list_size);
    }
    if attribute_list_size == 0 {
        unsafe { (api.close_pseudo_console)(pseudo_console) };
        close_handle_if_valid(con_reader);
        close_handle_if_valid(con_writer);
        return Err(anyhow::anyhow!(
            "InitializeProcThreadAttributeList size lookup failed"
        ));
    }

    let mut attribute_list_storage = vec![0u8; attribute_list_size];
    let attribute_list = attribute_list_storage.as_mut_ptr() as LPPROC_THREAD_ATTRIBUTE_LIST;
    let init_attr_ok = unsafe {
        InitializeProcThreadAttributeList(attribute_list, 1, 0, &mut attribute_list_size)
    };
    if init_attr_ok == 0 {
        let err = unsafe { GetLastError() };
        unsafe { (api.close_pseudo_console)(pseudo_console) };
        close_handle_if_valid(con_reader);
        close_handle_if_valid(con_writer);
        return Err(anyhow::anyhow!(
            "InitializeProcThreadAttributeList failed: {err}"
        ));
    }

    let update_attr_ok = unsafe {
        UpdateProcThreadAttribute(
            attribute_list,
            0,
            PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE,
            pseudo_console as LPVOID,
            size_of::<HPCON>(),
            null_mut(),
            null_mut(),
        )
    };
    if update_attr_ok == 0 {
        let err = unsafe { GetLastError() };
        unsafe {
            DeleteProcThreadAttributeList(attribute_list);
            (api.close_pseudo_console)(pseudo_console);
        }
        close_handle_if_valid(con_reader);
        close_handle_if_valid(con_writer);
        return Err(anyhow::anyhow!("UpdateProcThreadAttribute failed: {err}"));
    }

    let mut command_wide: Vec<u16> = std::ffi::OsStr::new(command_line)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let mut startup_info: STARTUPINFOEXW = unsafe { zeroed() };
    startup_info.StartupInfo.cb = size_of::<STARTUPINFOEXW>() as u32;
    startup_info.StartupInfo.dwFlags |= STARTF_USESTDHANDLES;
    startup_info.StartupInfo.hStdInput = null_mut();
    startup_info.StartupInfo.hStdOutput = null_mut();
    startup_info.StartupInfo.hStdError = null_mut();
    startup_info.lpAttributeList = attribute_list;

    let mut process_info: PROCESS_INFORMATION = unsafe { zeroed() };
    let create_flags = CREATE_UNICODE_ENVIRONMENT | EXTENDED_STARTUPINFO_PRESENT;
    let create_ok = if let Some(token) = primary_token {
        unsafe {
            CreateProcessAsUserW(
                token,
                null(),
                command_wide.as_mut_ptr(),
                null_mut(),
                null_mut(),
                FALSE,
                create_flags,
                null_mut(),
                null(),
                &mut startup_info.StartupInfo,
                &mut process_info,
            )
        }
    } else {
        unsafe {
            CreateProcessW(
                null(),
                command_wide.as_mut_ptr(),
                null_mut(),
                null_mut(),
                FALSE,
                create_flags,
                null_mut(),
                null(),
                &mut startup_info.StartupInfo,
                &mut process_info,
            )
        }
    };

    unsafe {
        DeleteProcThreadAttributeList(attribute_list);
    }

    if create_ok == 0 {
        let err = unsafe { GetLastError() };
        unsafe { (api.close_pseudo_console)(pseudo_console) };
        close_handle_if_valid(con_writer);
        close_handle_if_valid(con_reader);
        let api_name = if primary_token.is_some() {
            "CreateProcessAsUserW"
        } else {
            "CreateProcessW"
        };
        return Err(anyhow::anyhow!("{api_name} failed: {err}"));
    }

    let stdin_write_file = unsafe { std::fs::File::from_raw_handle(con_writer as RawHandle) };
    let stdout_read_file = unsafe { std::fs::File::from_raw_handle(con_reader as RawHandle) };

    Ok(UserShellProcess {
        stdin_write: stdin_write_file,
        stdout_read: stdout_read_file,
        process_handle: process_info.hProcess,
        thread_handle: process_info.hThread,
        console_handle: pseudo_console,
    })
}

// ---------------------------------------------------------------------------
// Relay-backed shell transport (agent outbound TCP 443)
// ---------------------------------------------------------------------------

pub async fn run_shell_relay_session(
    session_id: String,
    token: String,
    run_as: ShellRunAs,
    target_session_id: Option<u32>,
    relay_url: String,
    e2e_key_b64: String,
) -> Result<()> {
    let mut process = spawn_shell_process(session_id.clone(), run_as, target_session_id).await?;
    let relay_target = parse_relay_target(&relay_url)?;
    let addr = format!("{}:{}", relay_target.host, relay_target.port);
    let connect_timeout = Duration::from_secs(
        std::env::var("RMM_RELAY_CONNECT_TIMEOUT_SECS")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(10),
    );
    let tcp_stream = tokio::time::timeout(connect_timeout, TcpStream::connect(addr))
        .await
        .map_err(|_| anyhow::anyhow!("connect shell relay tcp timed out"))?
        .context("connect shell relay tcp")?;
    tcp_stream
        .set_nodelay(true)
        .context("set shell relay TCP_NODELAY")?;

    let tls_config = build_relay_client_tls_config(None, None)?;
    let connector = TlsConnector::from(Arc::new(tls_config));
    let server_name =
        ServerName::try_from(relay_target.host.clone()).context("build relay server name")?;
    let mut stream =
        tokio::time::timeout(connect_timeout, connector.connect(server_name, tcp_stream))
            .await
            .map_err(|_| anyhow::anyhow!("shell relay tls connect timed out"))?
            .context("shell relay tls connect")?;

    let request = format!(
        "GET /relay/{session_id} HTTP/1.1\r\nHost: {host}\r\n\r\n",
        session_id = session_id,
        host = relay_target.host
    );
    stream
        .write_all(request.as_bytes())
        .await
        .context("write shell relay request")?;
    tokio::time::timeout(connect_timeout, read_http_response(&mut stream))
        .await
        .map_err(|_| anyhow::anyhow!("read shell relay response timed out"))??;

    let key_bytes = BASE64_URL_SAFE_NO_PAD
        .decode(e2e_key_b64.trim())
        .or_else(|_| BASE64_STANDARD.decode(e2e_key_b64.trim()))
        .context("decode shell relay e2e key")?;
    let cipher = build_e2e_cipher(&key_bytes)?;

    let mut hello_counter = 0u64;
    write_e2e_frame_flush(&mut stream, &cipher, &mut hello_counter, b"hello-world").await?;

    let (relay_reader, relay_writer) = tokio::io::split(stream);
    let writer_cipher = build_e2e_cipher(&key_bytes)?;
    let (relay_out_tx, relay_out_rx) = mpsc::channel::<Vec<u8>>(OUTPUT_CHANNEL_CAP * 2);
    let writer_handle = tokio::spawn(run_shell_relay_writer(
        relay_writer,
        writer_cipher,
        relay_out_rx,
    ));

    let pty_reader = process
        .output()
        .map_err(|e| anyhow::anyhow!("failed to get shell output pipe: {e}"))?;
    let pty_writer = process
        .input()
        .map_err(|e| anyhow::anyhow!("failed to get shell input pipe: {e}"))?;

    let (output_tx, mut output_rx) = mpsc::channel::<Vec<u8>>(OUTPUT_CHANNEL_CAP);
    let sid_out = session_id.clone();
    let output_handle = tokio::task::spawn_blocking(move || {
        let mut reader = pty_reader;
        let mut buf = [0u8; PTY_READ_BUF];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    if output_tx.blocking_send(buf[..n].to_vec()).is_err() {
                        break;
                    }
                }
                Err(e) => {
                    debug!(session_id = %sid_out, error = %e, "ConPTY stdout read error");
                    break;
                }
            }
        }
    });

    let (input_tx, mut input_rx) = mpsc::channel::<Vec<u8>>(INPUT_CHANNEL_CAP);
    let sid_in = session_id.clone();
    let input_handle = tokio::task::spawn_blocking(move || {
        let mut writer = pty_writer;
        while let Some(data) = input_rx.blocking_recv() {
            if let Err(e) = writer.write_all(&data) {
                debug!(session_id = %sid_in, error = %e, "ConPTY stdin write error");
                break;
            }
        }
    });

    let (relay_in_tx, mut relay_in_rx) =
        mpsc::channel::<std::result::Result<Vec<u8>, String>>(INPUT_CHANNEL_CAP * 2);
    let reader_handle = tokio::spawn(run_shell_relay_reader(
        relay_reader,
        cipher,
        relay_in_tx,
        session_id.clone(),
    ));

    let mut authed = false;
    loop {
        tokio::select! {
            output = output_rx.recv() => {
                match output {
                    Some(data) => {
                        let frame = build_shell_frame(SHELL_MSG_OUTPUT, &data)
                            .map_err(|e| anyhow::anyhow!("build output frame: {e}"))?;
                        if relay_out_tx.send(frame).await.is_err() {
                            break;
                        }
                    }
                    None => {
                        break;
                    }
                }
            }
            relay_payload = relay_in_rx.recv() => {
                let payload = match relay_payload {
                    Some(Ok(payload)) => payload,
                    Some(Err(err)) => {
                        debug!(session_id = %session_id, error = %err, "shell relay read ended");
                        break;
                    }
                    None => break,
                };
                if payload == RELAY_HEARTBEAT_PAYLOAD || payload == b"hello-world" {
                    continue;
                }
                let (message_type, frame_payload) = match parse_shell_wire_frame(&payload) {
                    Ok(value) => value,
                    Err(err) => {
                        warn!(session_id = %session_id, error = %err, "invalid shell relay frame");
                        continue;
                    }
                };

                match message_type {
                    SHELL_MSG_AUTH => {
                        let received = String::from_utf8_lossy(frame_payload);
                        if received == token {
                            authed = true;
                            debug!(session_id = %session_id, "shell relay auth succeeded");
                        } else {
                            let frame = build_shell_frame(SHELL_MSG_ERROR, b"authentication failed")
                                .map_err(|e| anyhow::anyhow!("build auth error frame: {e}"))?;
                            let _ = relay_out_tx.send(frame).await;
                            break;
                        }
                    }
                    SHELL_MSG_INPUT => {
                        if !authed {
                            warn!(session_id = %session_id, "shell relay input before auth ignored");
                            continue;
                        }
                        if input_tx.send(frame_payload.to_vec()).await.is_err() {
                            break;
                        }
                    }
                    SHELL_MSG_RESIZE => {
                        if !authed {
                            warn!(session_id = %session_id, "shell relay resize before auth ignored");
                            continue;
                        }
                        if let Some((cols, rows)) = parse_shell_resize_payload(frame_payload) {
                            if let Err(err) = process.resize(cols as i16, rows as i16) {
                                warn!(session_id = %session_id, cols, rows, error = %err, "ConPTY resize failed");
                            }
                        }
                    }
                    other => {
                        warn!(session_id = %session_id, msg_type = other, "unexpected shell relay message type");
                    }
                }
            }
        }
    }

    let exit_code = if process.is_alive() {
        let _ = process.exit(0);
        0u32
    } else {
        process.wait(Some(0)).unwrap_or(1)
    };
    let exit_payload = build_shell_exit_payload(exit_code);
    if let Ok(exit_frame) = build_shell_frame(SHELL_MSG_EXIT, &exit_payload) {
        let _ = relay_out_tx.send(exit_frame).await;
    }

    drop(input_tx);
    drop(relay_out_tx);
    reader_handle.abort();
    let _ = tokio::time::timeout(Duration::from_secs(5), output_handle).await;
    let _ = tokio::time::timeout(Duration::from_secs(5), input_handle).await;
    let _ = tokio::time::timeout(Duration::from_secs(5), writer_handle).await;

    if exit_code != 0 {
        warn!(
            session_id = %session_id,
            exit_code = exit_code,
            "shell relay session ended with non-zero exit"
        );
    } else {
        debug!(
            session_id = %session_id,
            exit_code = exit_code,
            "shell relay session ended"
        );
    }
    Ok(())
}

fn parse_shell_wire_frame(frame: &[u8]) -> Result<(u8, &[u8])> {
    if frame.len() < 3 {
        return Err(anyhow::anyhow!("shell frame too short"));
    }
    let msg_type = frame[0];
    let len = u16::from_be_bytes([frame[1], frame[2]]) as usize;
    if frame.len() != 3 + len {
        return Err(anyhow::anyhow!("shell frame length mismatch"));
    }
    Ok((msg_type, &frame[3..]))
}

async fn run_shell_relay_writer<W>(
    mut writer: W,
    cipher: ChaCha20Poly1305,
    mut outgoing_rx: mpsc::Receiver<Vec<u8>>,
) where
    W: AsyncWriteExt + Unpin + Send,
{
    let mut send_counter = 1u64; // 0 is reserved for hello-world
    let mut ticker = interval(Duration::from_secs(RELAY_HEARTBEAT_INTERVAL_SECS));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            _ = ticker.tick() => {
                if let Err(err) = write_e2e_frame_flush(&mut writer, &cipher, &mut send_counter, RELAY_HEARTBEAT_PAYLOAD).await {
                    debug!(error = %err, "shell relay heartbeat write failed");
                    break;
                }
            }
            payload = outgoing_rx.recv() => {
                match payload {
                    Some(payload) => {
                        if let Err(err) = write_e2e_frame_flush(&mut writer, &cipher, &mut send_counter, &payload).await {
                            debug!(error = %err, "shell relay payload write failed");
                            break;
                        }
                    }
                    None => break,
                }
            }
        }
    }
}

async fn run_shell_relay_reader<R>(
    mut reader: R,
    cipher: ChaCha20Poly1305,
    incoming_tx: mpsc::Sender<std::result::Result<Vec<u8>, String>>,
    session_id: String,
) where
    R: AsyncRead + Unpin,
{
    loop {
        match read_e2e_frame_from(&mut reader, &cipher).await {
            Ok(payload) => {
                if incoming_tx.send(Ok(payload)).await.is_err() {
                    break;
                }
            }
            Err(err) => {
                let message = err.to_string();
                let _ = incoming_tx.send(Err(message.clone())).await;
                debug!(
                    session_id = %session_id,
                    error = %message,
                    "shell relay reader stopped"
                );
                break;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// QUIC-backed shell transport
// ---------------------------------------------------------------------------

/// Accept one QUIC shell connection, authenticate, and bridge I/O to the
/// shared process channels. Returns once the session ends.
pub async fn accept_shell_quic_connection(
    endpoint: quinn::Endpoint,
    token: String,
    shell_io: Arc<tokio::sync::Mutex<Option<SharedShellIo>>>,
    session_id: String,
) {
    let idle_timeout = Duration::from_secs(
        std::env::var("RMM_SHELL_QUIC_ACCEPT_IDLE_TIMEOUT_SECS")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(180),
    );
    let poll_timeout = Duration::from_secs(5);
    let started_at = Instant::now();

    loop {
        if shell_io.lock().await.is_none() {
            debug!(
                session_id = %session_id,
                "shell quic accept loop ending because another transport claimed process I/O"
            );
            break;
        }

        let connecting = match tokio::time::timeout(poll_timeout, endpoint.accept()).await {
            Ok(Some(connecting)) => connecting,
            Ok(None) => break,
            Err(_) => {
                if started_at.elapsed() >= idle_timeout {
                    let stale_io = {
                        let mut guard = shell_io.lock().await;
                        guard.take()
                    };
                    if let Some(io) = stale_io {
                        terminate_shared_shell_io(io).await;
                    }
                    warn!(
                        session_id = %session_id,
                        timeout_secs = idle_timeout.as_secs(),
                        "shell quic accept loop timed out before a transport claimed process I/O"
                    );
                    break;
                }
                continue;
            }
        };

        let connection = match connecting.await {
            Ok(conn) => conn,
            Err(err) => {
                warn!(session_id = %session_id, error = %err, "shell quic connection failed");
                continue;
            }
        };
        debug!(
            session_id = %session_id,
            remote = %connection.remote_address(),
            "shell quic connection accepted"
        );

        let (mut send, mut recv) = match connection.accept_bi().await {
            Ok(streams) => streams,
            Err(err) => {
                warn!(session_id = %session_id, error = %err, "shell quic bi-stream accept failed");
                continue;
            }
        };

        // Authenticate: first frame must be SHELL_MSG_AUTH with the correct token.
        match read_shell_frame(&mut recv).await {
            Ok(frame) if frame.message_type == SHELL_MSG_AUTH => {
                let received = String::from_utf8_lossy(&frame.payload);
                if received != token {
                    warn!(session_id = %session_id, "shell quic auth token mismatch");
                    let _ = write_shell_frame(&mut send, SHELL_MSG_ERROR, b"authentication failed")
                        .await;
                    let _ = send.finish();
                    continue;
                }
                debug!(session_id = %session_id, "shell quic auth succeeded");
            }
            Ok(frame) => {
                warn!(session_id = %session_id, msg_type = frame.message_type, "expected auth frame on quic stream");
                let _ = send.finish();
                continue;
            }
            Err(err) => {
                warn!(session_id = %session_id, error = %err, "shell quic read auth failed");
                continue;
            }
        }

        // Claim the shared process I/O.
        let io = {
            let mut guard = shell_io.lock().await;
            guard.take()
        };
        let Some(io) = io else {
            warn!(session_id = %session_id, "shell process already claimed by another transport");
            let _ =
                write_shell_frame(&mut send, SHELL_MSG_ERROR, b"session already connected").await;
            let _ = send.finish();
            continue;
        };

        debug!(session_id = %session_id, "shell quic transport claimed process I/O");
        run_shell_quic_bridge(session_id.clone(), send, recv, io).await;
        break;
    }
}

async fn terminate_shared_shell_io(io: SharedShellIo) {
    let SharedShellIo { process, .. } = io;
    let mut proc = process.lock().await;
    if proc.is_alive() {
        let _ = proc.exit(0);
    }
}

async fn run_shell_quic_bridge(
    session_id: String,
    mut send: quinn::SendStream,
    recv: quinn::RecvStream,
    io: SharedShellIo,
) {
    let SharedShellIo {
        mut output_rx,
        input_tx,
        process,
    } = io;

    let (quic_in_tx, mut quic_in_rx) = mpsc::channel::<
        std::result::Result<talos_protocol::ShellFrame, String>,
    >(INPUT_CHANNEL_CAP * 2);
    let reader_handle = tokio::spawn(run_shell_quic_reader(recv, quic_in_tx, session_id.clone()));

    let sid = session_id.clone();
    let bridge_result: Result<(), anyhow::Error> = async {
        loop {
            tokio::select! {
                output = output_rx.recv() => {
                    match output {
                        Some(data) => {
                            write_shell_frame(&mut send, SHELL_MSG_OUTPUT, &data).await
                                .context("write quic output frame")?;
                        }
                        None => {
                            debug!(session_id = %sid, "quic output channel closed");
                            break;
                        }
                    }
                }
                frame_result = quic_in_rx.recv() => {
                    match frame_result {
                        Some(Ok(frame)) => {
                            match frame.message_type {
                                SHELL_MSG_INPUT => {
                                    if input_tx.send(frame.payload).await.is_err() {
                                        debug!(session_id = %sid, "quic input channel closed");
                                        break;
                                    }
                                }
                                SHELL_MSG_RESIZE => {
                                    if let Some((cols, rows)) = parse_shell_resize_payload(&frame.payload) {
                                        let mut proc = process.lock().await;
                                        if let Err(err) = proc.resize(cols as i16, rows as i16) {
                                            warn!(session_id = %sid, error = %err, "ConPTY resize failed (quic)");
                                        }
                                    }
                                }
                                _ => {
                                    warn!(session_id = %sid, msg_type = frame.message_type, "unexpected shell quic frame type");
                                }
                            }
                        }
                        Some(Err(err)) => {
                            debug!(session_id = %sid, error = %err, "quic read ended");
                            break;
                        }
                        None => break,
                    }
                }
            }
        }
        Ok(())
    }
    .await;

    if let Err(err) = &bridge_result {
        warn!(session_id = %session_id, error = %err, "shell quic bridge error");
    }

    let exit_code = {
        let mut proc = process.lock().await;
        if proc.is_alive() {
            let _ = proc.exit(0);
            0u32
        } else {
            proc.wait(Some(0)).unwrap_or(1)
        }
    };

    let exit_payload = build_shell_exit_payload(exit_code);
    let _ = write_shell_frame(&mut send, SHELL_MSG_EXIT, &exit_payload).await;
    let _ = send.finish();
    reader_handle.abort();

    if exit_code != 0 {
        warn!(
            session_id = %session_id,
            exit_code,
            "shell quic session ended with non-zero exit"
        );
    } else {
        debug!(session_id = %session_id, exit_code, "shell quic session ended");
    }
}

async fn run_shell_quic_reader(
    mut recv: quinn::RecvStream,
    incoming_tx: mpsc::Sender<std::result::Result<talos_protocol::ShellFrame, String>>,
    session_id: String,
) {
    loop {
        match read_shell_frame(&mut recv).await {
            Ok(frame) => {
                if incoming_tx.send(Ok(frame)).await.is_err() {
                    break;
                }
            }
            Err(err) => {
                let message = err.to_string();
                let _ = incoming_tx.send(Err(message.clone())).await;
                debug!(
                    session_id = %session_id,
                    error = %message,
                    "shell quic reader stopped"
                );
                break;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Multi-transport shell session (QUIC + relay racing)
// ---------------------------------------------------------------------------

/// Shared shell process I/O that can be claimed by the first transport to
/// authenticate.
pub struct SharedShellIo {
    pub output_rx: mpsc::Receiver<Vec<u8>>,
    pub input_tx: mpsc::Sender<Vec<u8>>,
    pub process: Arc<tokio::sync::Mutex<ShellProcess>>,
}

/// Spawn a shell process and set up shared I/O channels. Both relay and QUIC
/// transports race to claim the I/O.
pub async fn start_shell_with_shared_io(
    session_id: String,
    run_as: ShellRunAs,
    target_session_id: Option<u32>,
) -> Result<Arc<tokio::sync::Mutex<Option<SharedShellIo>>>> {
    let mut process = spawn_shell_process(session_id.clone(), run_as, target_session_id).await?;

    let pty_reader = process
        .output()
        .map_err(|e| anyhow::anyhow!("get shell output pipe: {e}"))?;
    let pty_writer = process
        .input()
        .map_err(|e| anyhow::anyhow!("get shell input pipe: {e}"))?;

    let (output_tx, output_rx) = mpsc::channel::<Vec<u8>>(OUTPUT_CHANNEL_CAP);
    let (input_tx, mut input_rx) = mpsc::channel::<Vec<u8>>(INPUT_CHANNEL_CAP);

    let sid_out = session_id.clone();
    tokio::task::spawn_blocking(move || {
        let mut reader = pty_reader;
        let mut buf = [0u8; PTY_READ_BUF];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    if output_tx.blocking_send(buf[..n].to_vec()).is_err() {
                        break;
                    }
                }
                Err(e) => {
                    debug!(session_id = %sid_out, error = %e, "ConPTY stdout read error (shared)");
                    break;
                }
            }
        }
    });

    let sid_in = session_id.clone();
    tokio::task::spawn_blocking(move || {
        let mut writer = pty_writer;
        while let Some(data) = input_rx.blocking_recv() {
            if let Err(e) = writer.write_all(&data) {
                debug!(session_id = %sid_in, error = %e, "ConPTY stdin write error (shared)");
                break;
            }
        }
    });

    let process = Arc::new(tokio::sync::Mutex::new(process));
    let io = SharedShellIo {
        output_rx,
        input_tx,
        process,
    };

    Ok(Arc::new(tokio::sync::Mutex::new(Some(io))))
}

/// Run the relay transport path for a shared-IO shell session.
/// Claims the shared I/O on successful auth, then bridges relay ↔ process.
pub async fn run_shell_relay_shared(
    session_id: String,
    token: String,
    relay_url: String,
    e2e_key_b64: String,
    shell_io: Arc<tokio::sync::Mutex<Option<SharedShellIo>>>,
) -> Result<()> {
    let relay_target = parse_relay_target(&relay_url)?;
    let addr = format!("{}:{}", relay_target.host, relay_target.port);
    let connect_timeout = Duration::from_secs(
        std::env::var("RMM_RELAY_CONNECT_TIMEOUT_SECS")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(10),
    );
    let tcp_stream = tokio::time::timeout(connect_timeout, TcpStream::connect(addr))
        .await
        .map_err(|_| anyhow::anyhow!("connect shell relay tcp timed out"))?
        .context("connect shell relay tcp")?;
    tcp_stream
        .set_nodelay(true)
        .context("set shell relay TCP_NODELAY")?;

    let tls_config = build_relay_client_tls_config(None, None)?;
    let connector = TlsConnector::from(Arc::new(tls_config));
    let server_name =
        ServerName::try_from(relay_target.host.clone()).context("build relay server name")?;
    let mut stream =
        tokio::time::timeout(connect_timeout, connector.connect(server_name, tcp_stream))
            .await
            .map_err(|_| anyhow::anyhow!("shell relay tls connect timed out"))?
            .context("shell relay tls connect")?;

    let request = format!(
        "GET /relay/{session_id} HTTP/1.1\r\nHost: {host}\r\n\r\n",
        session_id = session_id,
        host = relay_target.host
    );
    stream
        .write_all(request.as_bytes())
        .await
        .context("write shell relay request")?;
    tokio::time::timeout(connect_timeout, read_http_response(&mut stream))
        .await
        .map_err(|_| anyhow::anyhow!("read shell relay response timed out"))??;

    let key_bytes = BASE64_URL_SAFE_NO_PAD
        .decode(e2e_key_b64.trim())
        .or_else(|_| BASE64_STANDARD.decode(e2e_key_b64.trim()))
        .context("decode shell relay e2e key")?;
    let cipher = build_e2e_cipher(&key_bytes)?;

    let mut hello_counter = 0u64;
    write_e2e_frame_flush(&mut stream, &cipher, &mut hello_counter, b"hello-world").await?;

    let (mut relay_reader, relay_writer) = tokio::io::split(stream);
    let writer_cipher = build_e2e_cipher(&key_bytes)?;
    let (relay_out_tx, relay_out_rx) = mpsc::channel::<Vec<u8>>(OUTPUT_CHANNEL_CAP * 2);
    let writer_handle = tokio::spawn(run_shell_relay_writer(
        relay_writer,
        writer_cipher,
        relay_out_rx,
    ));

    // Wait for viewer auth over relay before claiming process I/O.
    let io = loop {
        let payload = match read_e2e_frame_from(&mut relay_reader, &cipher).await {
            Ok(p) => p,
            Err(err) => {
                debug!(session_id = %session_id, error = %err, "shell relay read ended before auth");
                writer_handle.abort();
                return Ok(());
            }
        };
        if payload == RELAY_HEARTBEAT_PAYLOAD || payload == b"hello-world" {
            continue;
        }
        let (message_type, frame_payload) = match parse_shell_wire_frame(&payload) {
            Ok(v) => v,
            Err(err) => {
                warn!(session_id = %session_id, error = %err, "invalid shell relay frame (pre-auth)");
                continue;
            }
        };
        if message_type == SHELL_MSG_AUTH {
            let received = String::from_utf8_lossy(frame_payload);
            if received == token {
                debug!(session_id = %session_id, "shell relay auth succeeded (shared)");
                let io = {
                    let mut guard = shell_io.lock().await;
                    guard.take()
                };
                match io {
                    Some(io) => break io,
                    None => {
                        warn!(session_id = %session_id, "shell process already claimed by QUIC");
                        let frame =
                            build_shell_frame(SHELL_MSG_ERROR, b"session already connected")
                                .map_err(|e| anyhow::anyhow!("build error frame: {e}"))?;
                        let _ = relay_out_tx.send(frame).await;
                        writer_handle.abort();
                        return Ok(());
                    }
                }
            } else {
                let frame = build_shell_frame(SHELL_MSG_ERROR, b"authentication failed")
                    .map_err(|e| anyhow::anyhow!("build auth error frame: {e}"))?;
                let _ = relay_out_tx.send(frame).await;
                writer_handle.abort();
                return Ok(());
            }
        }
    };

    debug!(session_id = %session_id, "shell relay transport claimed process I/O");
    let SharedShellIo {
        mut output_rx,
        input_tx,
        process,
    } = io;

    let (relay_in_tx, mut relay_in_rx) =
        mpsc::channel::<std::result::Result<Vec<u8>, String>>(INPUT_CHANNEL_CAP * 2);
    let reader_handle = tokio::spawn(run_shell_relay_reader(
        relay_reader,
        cipher,
        relay_in_tx,
        session_id.clone(),
    ));

    // Main bridge loop (relay ↔ process)
    loop {
        tokio::select! {
            output = output_rx.recv() => {
                match output {
                    Some(data) => {
                        let frame = build_shell_frame(SHELL_MSG_OUTPUT, &data)
                            .map_err(|e| anyhow::anyhow!("build output frame: {e}"))?;
                        if relay_out_tx.send(frame).await.is_err() {
                            break;
                        }
                    }
                    None => break,
                }
            }
            relay_payload = relay_in_rx.recv() => {
                let payload = match relay_payload {
                    Some(Ok(payload)) => payload,
                    Some(Err(err)) => {
                        debug!(session_id = %session_id, error = %err, "shell relay read ended");
                        break;
                    }
                    None => break,
                };
                if payload == RELAY_HEARTBEAT_PAYLOAD || payload == b"hello-world" {
                    continue;
                }
                let (message_type, frame_payload) = match parse_shell_wire_frame(&payload) {
                    Ok(v) => v,
                    Err(err) => {
                        warn!(session_id = %session_id, error = %err, "invalid shell relay frame");
                        continue;
                    }
                };
                match message_type {
                    SHELL_MSG_INPUT => {
                        if input_tx.send(frame_payload.to_vec()).await.is_err() {
                            break;
                        }
                    }
                    SHELL_MSG_RESIZE => {
                        if let Some((cols, rows)) = parse_shell_resize_payload(frame_payload) {
                            let mut proc = process.lock().await;
                            if let Err(err) = proc.resize(cols as i16, rows as i16) {
                                warn!(session_id = %session_id, error = %err, "ConPTY resize failed (relay shared)");
                            }
                        }
                    }
                    _ => {
                        warn!(session_id = %session_id, msg_type = message_type, "unexpected shell relay message type");
                    }
                }
            }
        }
    }

    let exit_code = {
        let mut proc = process.lock().await;
        if proc.is_alive() {
            let _ = proc.exit(0);
            0u32
        } else {
            proc.wait(Some(0)).unwrap_or(1)
        }
    };
    if let Ok(exit_frame) = build_shell_frame(SHELL_MSG_EXIT, &build_shell_exit_payload(exit_code))
    {
        let _ = relay_out_tx.send(exit_frame).await;
    }

    drop(relay_out_tx);
    reader_handle.abort();
    let _ = tokio::time::timeout(Duration::from_secs(5), writer_handle).await;

    if exit_code != 0 {
        warn!(
            session_id = %session_id,
            exit_code,
            "shell relay shared session ended with non-zero exit"
        );
    } else {
        debug!(
            session_id = %session_id,
            exit_code,
            "shell relay shared session ended"
        );
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Frame I/O helpers (async, over tokio TcpStream halves)
// ---------------------------------------------------------------------------

/// Read one shell frame from an async reader.
///
/// Frame layout: `[1B type][2B payload length BE][payload]`.
async fn read_shell_frame<R: tokio::io::AsyncRead + Unpin>(
    reader: &mut R,
) -> Result<talos_protocol::ShellFrame> {
    let mut header = [0u8; 3];
    reader
        .read_exact(&mut header)
        .await
        .context("read shell frame header")?;

    let message_type = header[0];
    let length = u16::from_be_bytes([header[1], header[2]]) as usize;

    let mut payload = vec![0u8; length];
    if length > 0 {
        reader
            .read_exact(&mut payload)
            .await
            .context("read shell frame payload")?;
    }

    Ok(talos_protocol::ShellFrame {
        message_type,
        payload,
    })
}

/// Write one shell frame to an async writer.
async fn write_shell_frame<W: tokio::io::AsyncWrite + Unpin>(
    writer: &mut W,
    message_type: u8,
    payload: &[u8],
) -> Result<()> {
    let frame = build_shell_frame(message_type, payload)
        .map_err(|e| anyhow::anyhow!("build shell frame: {e}"))?;
    writer
        .write_all(&frame)
        .await
        .context("write shell frame")?;
    writer.flush().await.context("flush shell frame")?;
    Ok(())
}

#[cfg(all(test, target_os = "macos"))]
mod macos_shell_tests {
    use super::*;

    #[test]
    fn macos_effective_home_falls_back_for_empty_passwd_home() {
        assert_eq!(
            macos_effective_home("sebastian", "".to_string()),
            "/Users/sebastian"
        );
        assert_eq!(macos_effective_home("", "".to_string()), "/");
    }

    #[test]
    fn macos_effective_home_preserves_passwd_home() {
        assert_eq!(
            macos_effective_home("sebastian", "/Users/custom".to_string()),
            "/Users/custom"
        );
    }

    #[test]
    fn macos_effective_shell_uses_fallback_for_empty_or_noninteractive_shells() {
        let fallback = fallback_macos_shell();

        assert_eq!(macos_effective_shell(None), fallback);
        assert_eq!(macos_effective_shell(Some("".to_string())), fallback);
        assert_eq!(
            macos_effective_shell(Some("/usr/bin/false".to_string())),
            fallback
        );
        assert_eq!(
            macos_effective_shell(Some("/usr/sbin/nologin".to_string())),
            fallback
        );
        assert_eq!(
            macos_effective_shell(Some("/definitely/not/a/talos/shell".to_string())),
            fallback
        );
    }

    #[test]
    fn macos_effective_shell_preserves_existing_interactive_shell() {
        assert_eq!(
            macos_effective_shell(Some("/bin/sh".to_string())),
            "/bin/sh"
        );
    }
}
