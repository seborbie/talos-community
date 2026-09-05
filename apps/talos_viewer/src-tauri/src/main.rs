// Keep the viewer background-resident on Windows: do not spawn a console window
// by default. When debugging is explicitly enabled, we allocate/attach a console
// at runtime instead (see `init_debug_console()`).
#![cfg_attr(windows, windows_subsystem = "windows")]
#![cfg_attr(
    test,
    expect(
        clippy::items_after_test_module,
        reason = "the Tauri entry point remains last so the generated handler list stays adjacent to startup"
    )
)]

#[cfg(windows)]
use std::num::NonZeroIsize;
#[cfg(windows)]
use std::sync::atomic::AtomicU16;
use std::{
    collections::{HashMap, VecDeque},
    env,
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    net::{Ipv4Addr, SocketAddr, ToSocketAddrs, UdpSocket},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex, OnceLock,
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

#[cfg(target_os = "macos")]
use std::process::Command;

use anyhow::{anyhow, Context};
use base64::engine::general_purpose::{
    STANDARD as BASE64_STANDARD, URL_SAFE_NO_PAD as BASE64_URL_SAFE_NO_PAD,
};
use base64::Engine as _;
use chacha20poly1305::ChaCha20Poly1305;
use get_if_addrs::{get_if_addrs, IfAddr};
use quinn::crypto::rustls::QuicClientConfig;
use quinn::Connection;
use quinn::Endpoint;
use quinn::EndpointConfig;
use quinn::TokioRuntime;
use reqwest::Client;
use rustls::pki_types::ServerName;
use rustls::ClientConfig;
use rustls_pemfile::certs;
use serde::{Deserialize, Serialize};
use stunclient::StunClient;
use tauri::{
    AppHandle, Emitter, Manager, PhysicalPosition, PhysicalSize, Position, Size, State, WebviewUrl,
    WebviewWindow, WebviewWindowBuilder, Window, WindowEvent,
};
use tokio::task::JoinHandle;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    sync::{mpsc, oneshot, Mutex as AsyncMutex},
    time::{interval, sleep, timeout},
};
use tokio_rustls::TlsConnector;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};
use tracing_subscriber::fmt::MakeWriter;
use walkdir::WalkDir;
use zip::{write::SimpleFileOptions, CompressionMethod, ZipArchive, ZipWriter};

#[cfg(windows)]
use raw_window_handle::{
    DisplayHandle, HandleError, HasDisplayHandle, HasWindowHandle, RawDisplayHandle,
    RawWindowHandle, Win32WindowHandle, WindowHandle, WindowsDisplayHandle,
};
#[cfg(windows)]
use softbuffer::{Context as SoftbufferContext, Surface as SoftbufferSurface};
#[cfg(windows)]
use std::os::windows::ffi::OsStrExt;
#[cfg(windows)]
use windows_sys::Win32::Foundation::{HWND, POINT, RECT};
#[cfg(windows)]
use windows_sys::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
#[cfg(windows)]
use windows_sys::Win32::Graphics::Gdi::{
    ClientToScreen, CombineRgn, CreateRectRgn, DeleteObject, SetWindowRgn, RGN_DIFF,
};
#[cfg(windows)]
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
#[cfg(windows)]
use windows_sys::Win32::System::Console::{
    AllocConsole, AttachConsole, SetStdHandle, ATTACH_PARENT_PROCESS, STD_ERROR_HANDLE,
    STD_INPUT_HANDLE, STD_OUTPUT_HANDLE,
};
#[cfg(windows)]
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
#[cfg(windows)]
use windows_sys::Win32::System::Memory::{GlobalLock, GlobalUnlock};
#[cfg(windows)]
use windows_sys::Win32::System::SystemInformation::GetTickCount64;
#[cfg(windows)]
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, GetClientRect, GetWindowLongPtrW, RegisterClassW,
    SetWindowPos, ShowWindow, CS_HREDRAW, CS_VREDRAW, GWLP_HINSTANCE, HWND_TOP, SWP_NOACTIVATE,
    SWP_SHOWWINDOW, SW_HIDE, WM_KEYDOWN, WM_KEYUP, WM_KILLFOCUS, WM_LBUTTONDOWN, WM_LBUTTONUP,
    WM_MBUTTONDOWN, WM_MBUTTONUP, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_NCDESTROY, WM_RBUTTONDOWN,
    WM_RBUTTONUP, WM_SETFOCUS, WM_SYSKEYDOWN, WM_SYSKEYUP, WNDCLASSW, WS_POPUP, WS_VISIBLE,
};

// Windows key suppression via RIDEV_NOHOTKEYS (see native/win_key_block.cpp).
// The low-level keyboard hook has been removed; RIDEV_NOHOTKEYS is the sole
// mechanism, inherently scoped to viewport focus.
#[cfg(windows)]
extern "C" {
    fn win_key_block_init() -> i32;
    fn win_key_block_set_enabled(enabled: i32);
    /// Register RIDEV_NOHOTKEYS on a keyboard Raw Input device targeting `hwnd`.
    /// Suppresses system hotkeys (including Win key) while that window has focus.
    fn win_key_register_nohotkeys(hwnd: HWND) -> i32;
    /// Deregister the Raw Input keyboard device (RIDEV_REMOVE).
    fn win_key_deregister_nohotkeys() -> i32;
}

#[cfg(windows)]
mod display_delta;
#[cfg(windows)]
mod display_processing;
#[cfg(windows)]
mod mf_h264;
mod updater;
mod viewer_chat;
#[cfg(not(windows))]
mod viewport_cpu;
#[cfg(windows)]
mod viewport_d3d11;
#[cfg(target_os = "macos")]
mod viewport_macos;
#[cfg(windows)]
mod viewport_video;
#[cfg(target_os = "macos")]
mod vt_h264;
#[cfg(not(windows))]
use viewport_cpu::{parse_ivf_header, DecodedFrame, ModernDisplayCompositor, Vp8Decoder};
#[cfg(target_os = "macos")]
use viewport_macos::ViewportState as MacViewportState;
#[cfg(windows)]
use viewport_video::{
    parse_ivf_header, present_cached_frame, present_decoded_frame, CachedFrame, Vp8Decoder,
};

#[cfg(not(windows))]
#[derive(Clone)]
struct NonWindowsCachedFrame {
    width: u32,
    height: u32,
    argb: Vec<u32>,
}

#[cfg(not(windows))]
static NON_WINDOWS_REMOTE_DESKTOP_FRAMES: OnceLock<Mutex<HashMap<String, NonWindowsCachedFrame>>> =
    OnceLock::new();

const MAIN_WINDOW_LABEL: &str = "main";
const SESSION_WINDOW_PREFIX: &str = "session-";
const TRAY_ID: &str = "talos_viewer_tray";
const TRAY_MENU_EXIT_ID: &str = "tray_exit";
const TRAY_MENU_AUTOSTART_ID: &str = "tray_autostart";
const TRAY_MENU_ABOUT_ID: &str = "tray_about";
const TRAY_MENU_CHECK_UPDATES_ID: &str = "tray_check_updates";
#[cfg(any(windows, target_os = "macos"))]
const VIEWER_START_ON_LOGIN_ARG: &str = "--autostart";
const VIEWER_COPYRIGHT: &str = "Copyright (c) 2026 Talos";
const VIEWER_ABOUT_TITLE: &str = "About Talos Viewer";
const MAX_LEGACY_VP8_PAYLOAD_LEN: usize = 128 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LegacyVp8PayloadPrefix {
    Payload(usize),
    MidstreamIvfHeader,
    TooLarge(usize),
}

fn parse_legacy_vp8_payload_prefix(len_bytes: [u8; 4]) -> LegacyVp8PayloadPrefix {
    if len_bytes == *b"DKIF" {
        return LegacyVp8PayloadPrefix::MidstreamIvfHeader;
    }
    let payload_len = u32::from_le_bytes(len_bytes) as usize;
    if payload_len > MAX_LEGACY_VP8_PAYLOAD_LEN {
        return LegacyVp8PayloadPrefix::TooLarge(payload_len);
    }
    LegacyVp8PayloadPrefix::Payload(payload_len)
}

/// Outer size = most of the monitor work area (physical pixels) for the monitor
/// this window is on, falling back to the primary display, then 1280×800 if unknown.
fn apply_session_window_size(window: &WebviewWindow) {
    let monitor = window
        .primary_monitor()
        .ok()
        .flatten()
        .or_else(|| window.current_monitor().ok().flatten());
    let (width, height, position) = if let Some(monitor) = monitor {
        let area = monitor.work_area();
        let width = ((area.size.width as f64) * 0.9).round() as u32;
        let height = ((area.size.height as f64) * 0.9).round() as u32;
        let x = area.position.x + ((area.size.width.saturating_sub(width)) / 2) as i32;
        let y = area.position.y + ((area.size.height.saturating_sub(height)) / 2) as i32;
        (
            width.max(960).min(area.size.width),
            height.max(640).min(area.size.height),
            Some(PhysicalPosition { x, y }),
        )
    } else {
        (1280, 800, None)
    };
    if let Some(position) = position {
        let _ = window.set_position(Position::Physical(position));
    }
    let _ = window.set_size(Size::Physical(PhysicalSize { width, height }));
}

/// WebView2 handles F5 / Ctrl+R as reload **before** JS `keydown`; turn off browser accelerator keys.
#[cfg(windows)]
fn disable_browser_accelerator_keys(window: &WebviewWindow) {
    use webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2Settings3;
    use windows_core::Interface;

    if let Err(err) = window.with_webview(|platform| {
        let controller = platform.controller();
        unsafe {
            let Ok(core) = controller.CoreWebView2() else {
                return;
            };
            let Ok(settings) = core.Settings() else {
                return;
            };
            let Ok(settings3) = settings.cast::<ICoreWebView2Settings3>() else {
                return;
            };
            let _ = settings3.SetAreBrowserAcceleratorKeysEnabled(false);
        }
    }) {
        warn!(error = %err, "failed to disable WebView2 browser accelerator keys");
    }
}

#[cfg(not(windows))]
fn disable_browser_accelerator_keys(_window: &WebviewWindow) {}

#[cfg(target_os = "macos")]
fn allow_window_capture(window: &WebviewWindow) {
    let Ok(ns_window) = window.ns_window() else {
        return;
    };
    unsafe {
        let ns_window = &*(ns_window.cast::<objc2_app_kit::NSWindow>());
        ns_window.setSharingType(objc2_app_kit::NSWindowSharingType::ReadOnly);
    }
}

#[cfg(not(target_os = "macos"))]
fn allow_window_capture(_window: &WebviewWindow) {}

#[cfg(target_os = "macos")]
fn set_macos_activation_policy_regular() {
    let Some(mtm) = objc2::MainThreadMarker::new() else {
        return;
    };
    let app = objc2_app_kit::NSApplication::sharedApplication(mtm);
    let _ = app.setActivationPolicy(objc2_app_kit::NSApplicationActivationPolicy::Regular);
}

#[cfg(target_os = "macos")]
fn set_macos_activation_policy_accessory() {
    let Some(mtm) = objc2::MainThreadMarker::new() else {
        return;
    };
    let app = objc2_app_kit::NSApplication::sharedApplication(mtm);
    let _ = app.setActivationPolicy(objc2_app_kit::NSApplicationActivationPolicy::Accessory);
}

#[cfg(not(target_os = "macos"))]
fn set_macos_activation_policy_accessory() {}

#[cfg(target_os = "macos")]
fn activate_macos_app() {
    set_macos_activation_policy_regular();
    let Some(mtm) = objc2::MainThreadMarker::new() else {
        return;
    };
    let app = objc2_app_kit::NSApplication::sharedApplication(mtm);
    app.activate();
}

#[cfg(not(target_os = "macos"))]
fn activate_macos_app() {}

fn schedule_macos_accessory_if_no_session(app: AppHandle) {
    thread::spawn(move || {
        thread::sleep(Duration::from_millis(150));
        let app_for_main = app.clone();
        let _ = app.run_on_main_thread(move || {
            let has_session_window = app_for_main
                .webview_windows()
                .keys()
                .any(|label| label.starts_with(SESSION_WINDOW_PREFIX));
            if !has_session_window {
                set_macos_activation_policy_accessory();
            }
        });
    });
}

static SESSION_WINDOW_COUNTER: AtomicU64 = AtomicU64::new(1);
static VIEWER_LOG_PATH: OnceLock<PathBuf> = OnceLock::new();

fn about_dialog_message() -> String {
    format!(
        "Talos Viewer\nVersion {}\n\nRemote session viewer for the Talos RMM platform.\n\n{}",
        env!("CARGO_PKG_VERSION"),
        VIEWER_COPYRIGHT
    )
}

#[cfg(windows)]
fn show_message_box(title: &str, message: &str, flags: u32) -> i32 {
    use windows_sys::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_SETFOREGROUND, MB_TOPMOST};

    let title_wide: Vec<u16> = title.encode_utf16().chain(std::iter::once(0)).collect();
    let message_wide: Vec<u16> = message.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            message_wide.as_ptr(),
            title_wide.as_ptr(),
            flags | MB_SETFOREGROUND | MB_TOPMOST,
        )
    }
}

#[cfg(windows)]
fn show_info_dialog(title: &str, message: &str) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{MB_ICONINFORMATION, MB_OK};
    let _ = show_message_box(title, message, MB_OK | MB_ICONINFORMATION);
}

#[cfg(windows)]
fn show_error_dialog(title: &str, message: &str) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK};
    let _ = show_message_box(title, message, MB_OK | MB_ICONERROR);
}

#[cfg(windows)]
fn ask_yes_no_dialog(title: &str, message: &str) -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::{IDYES, MB_ICONQUESTION, MB_YESNO};
    show_message_box(title, message, MB_YESNO | MB_ICONQUESTION) == IDYES
}

#[cfg(not(windows))]
fn show_info_dialog(title: &str, message: &str) {
    show_macos_dialog(title, message, "OK", None);
}

#[cfg(not(windows))]
fn show_error_dialog(title: &str, message: &str) {
    show_macos_dialog(title, message, "OK", None);
}

#[cfg(not(windows))]
fn ask_yes_no_dialog(title: &str, message: &str) -> bool {
    show_macos_dialog(title, message, "Yes", Some("No"))
}

#[cfg(target_os = "macos")]
fn show_macos_dialog(
    title: &str,
    message: &str,
    ok_button: &str,
    cancel_button: Option<&str>,
) -> bool {
    let escaped_title = osascript_quote(title);
    let escaped_message = osascript_quote(message);
    let escaped_ok = osascript_quote(ok_button);
    let script = if let Some(cancel_button) = cancel_button {
        let escaped_cancel = osascript_quote(cancel_button);
        format!(
            r#"button returned of (display dialog "{message}" with title "{title}" buttons {{"{cancel}", "{ok}"}} default button "{ok}" cancel button "{cancel}")"#,
            message = escaped_message,
            title = escaped_title,
            cancel = escaped_cancel,
            ok = escaped_ok
        )
    } else {
        format!(
            r#"display dialog "{message}" with title "{title}" buttons {{"{ok}"}} default button "{ok}""#,
            message = escaped_message,
            title = escaped_title,
            ok = escaped_ok
        )
    };

    Command::new("osascript")
        .args(["-e", &script])
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

#[cfg(not(any(windows, target_os = "macos")))]
fn show_macos_dialog(
    _title: &str,
    _message: &str,
    _ok_button: &str,
    _cancel_button: Option<&str>,
) -> bool {
    false
}

#[cfg(target_os = "macos")]
fn osascript_quote(input: &str) -> String {
    input.replace('\\', "\\\\").replace('"', "\\\"")
}

fn show_about_dialog(app: &tauri::AppHandle) {
    #[cfg(windows)]
    {
        let _ = app;
        show_info_dialog(VIEWER_ABOUT_TITLE, &about_dialog_message());
    }

    #[cfg(not(windows))]
    {
        let _ = app;
        show_info_dialog(VIEWER_ABOUT_TITLE, &about_dialog_message());
    }
}

fn start_tray_update_check(
    app: tauri::AppHandle,
    check_updates_item: tauri::menu::MenuItem<tauri::Wry>,
) {
    let _ = check_updates_item.set_enabled(false);
    let update_manager = app.state::<ViewerUpdateState>().manager.clone();
    tauri::async_runtime::spawn(async move {
        let Some(manager) = update_manager else {
            show_error_dialog("Updates", "Viewer updater is unavailable.");
            let _ = check_updates_item.set_enabled(true);
            return;
        };

        let result = manager.manual_check(&app).await;
        let _ = check_updates_item.set_enabled(true);

        match result {
            Ok(check) if check.status == "update_ready" => {
                let version = check
                    .version
                    .unwrap_or_else(|| "an available release".to_string());
                let confirmed = ask_yes_no_dialog(
                    "Talos Viewer Update",
                    &format!(
                        "Version {version} is ready to install.\n\nThe viewer will close to complete the update.\n\nUpdate now?"
                    ),
                );
                if !confirmed {
                    return;
                }

                match manager.apply_staged_update(&app) {
                    Ok(true) => {}
                    Ok(false) => {
                        show_error_dialog(
                            "Talos Viewer Update",
                            "The update was prepared, but no staged package was available to launch.",
                        );
                    }
                    Err(err) => {
                        show_error_dialog("Talos Viewer Update", &err.to_string());
                    }
                }
            }
            Ok(_) => {
                show_info_dialog("Talos Viewer Update", "Talos Viewer is up to date.");
            }
            Err(err) => {
                show_error_dialog("Talos Viewer Update", &err.to_string());
            }
        }
    });
}

#[derive(Clone, Default)]
struct ViewerUpdateState {
    manager: Option<updater::UpdateManager>,
}

struct FileMakeWriter {
    file: Arc<Mutex<File>>,
}

impl FileMakeWriter {
    fn new(file: File) -> Self {
        Self {
            file: Arc::new(Mutex::new(file)),
        }
    }
}

struct FileWriterGuard<'a> {
    guard: std::sync::MutexGuard<'a, File>,
}

impl<'a> Write for FileWriterGuard<'a> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.guard.write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.guard.flush()
    }
}

impl<'a> MakeWriter<'a> for FileMakeWriter {
    type Writer = FileWriterGuard<'a>;

    fn make_writer(&'a self) -> Self::Writer {
        let guard = match self.file.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        FileWriterGuard { guard }
    }
}

fn debug_enabled() -> bool {
    std::env::var("RMM_DEBUG").ok().is_some_and(|v| {
        let v = v.trim();
        v.eq_ignore_ascii_case("debug") || v.eq_ignore_ascii_case("true")
    })
}

/// Loads `.env` from cwd first, then the executable directory (`dotenvy` does not override existing vars).
fn load_viewer_dotenv() {
    if let Ok(cwd) = std::env::current_dir() {
        let env_path = cwd.join(".env");
        if env_path.exists() {
            let _ = dotenvy::from_path(&env_path);
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let env_path = dir.join(".env");
            if env_path.exists() {
                let _ = dotenvy::from_path(&env_path);
            }
        }
    }
}

fn viewer_log_filter() -> tracing_subscriber::EnvFilter {
    tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        let debug_env = debug_enabled()
            || std::env::var("RMM_VIEWER")
                .ok()
                .or_else(|| std::env::var("RMM_AGENT").ok())
                .map(|v| v.trim().eq_ignore_ascii_case("debug"))
                .unwrap_or(false);
        if debug_env {
            tracing_subscriber::EnvFilter::new("debug,rmm_chat=trace")
        } else {
            tracing_subscriber::EnvFilter::new("info,rmm_chat=trace")
        }
    })
}

#[cfg(target_os = "windows")]
fn windows_log_path_candidates() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Ok(base) = std::env::var("PROGRAMDATA") {
        paths.push(
            PathBuf::from(base)
                .join("Talos")
                .join("logs")
                .join("talos_viewer.log"),
        );
    }
    paths.push(PathBuf::from(r"C:\ProgramData\Talos\logs\talos_viewer.log"));
    paths.push(std::env::temp_dir().join("talos_viewer.log"));
    paths.push(PathBuf::from(r"C:\Windows\Temp\talos_viewer.log"));
    paths
}

#[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
fn windows_log_path_candidates() -> Vec<PathBuf> {
    vec![std::env::temp_dir().join("talos_viewer.log")]
}

#[cfg(target_os = "macos")]
fn windows_log_path_candidates() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(home) = std::env::var_os("HOME") {
        paths.push(
            PathBuf::from(home)
                .join("Library")
                .join("Logs")
                .join("Talos")
                .join("talos_viewer.log"),
        );
    }
    paths.push(std::env::temp_dir().join("talos_viewer.log"));
    paths
}

fn resolve_log_path() -> PathBuf {
    for path in windows_log_path_candidates() {
        let parent_ok = path
            .parent()
            .map(|parent| fs::create_dir_all(parent).is_ok())
            .unwrap_or(true);
        if !parent_ok {
            continue;
        }
        if OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .is_ok()
        {
            return path;
        }
    }
    windows_log_path_candidates()
        .into_iter()
        .next()
        .unwrap_or_else(|| std::env::temp_dir().join("talos_viewer.log"))
}

fn viewer_log_path() -> PathBuf {
    VIEWER_LOG_PATH.get_or_init(resolve_log_path).clone()
}

fn init_file_logging() -> Result<(), std::io::Error> {
    let log_path = viewer_log_path();
    if let Some(parent) = log_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)?;
    let writer = FileMakeWriter::new(file);
    tracing_subscriber::fmt()
        .with_env_filter(viewer_log_filter())
        .with_timer(tracing_subscriber::fmt::time::SystemTime)
        .with_writer(writer)
        .with_ansi(false)
        .init();
    info!(path = %log_path.display(), "logging to file");
    debug!("viewer debug logging enabled");
    warn!("viewer file logging initialized");
    Ok(())
}

#[cfg(windows)]
fn to_wide_null(s: &str) -> Vec<u16> {
    std::ffi::OsStr::new(s)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

/// Conditionally allocates/attaches a Windows console for debugging.
///
/// Enabled when `RMM_DEBUG` is `debug` or `true` (case-insensitive).
#[cfg(windows)]
fn init_debug_console() {
    if !debug_enabled() {
        return;
    }

    unsafe {
        // Prefer using an existing console when launched from one.
        let attached = AttachConsole(ATTACH_PARENT_PROCESS) != 0;
        if !attached {
            let _ = AllocConsole();
        }

        // Redirect stdio handles to the console so logs show up.
        // If any of these fail, keep going; the console window itself is still useful.
        let conout = to_wide_null("CONOUT$");
        let conin = to_wide_null("CONIN$");

        let out_handle = CreateFileW(
            conout.as_ptr(),
            // GENERIC_READ | GENERIC_WRITE
            0x8000_0000u32 | 0x4000_0000u32,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            std::ptr::null_mut(),
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL,
            std::ptr::null_mut(),
        );
        if out_handle as isize != -1 {
            let _ = SetStdHandle(STD_OUTPUT_HANDLE, out_handle);
            let _ = SetStdHandle(STD_ERROR_HANDLE, out_handle);
        }

        let in_handle = CreateFileW(
            conin.as_ptr(),
            // GENERIC_READ
            0x8000_0000u32,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            std::ptr::null_mut(),
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL,
            std::ptr::null_mut(),
        );
        if in_handle as isize != -1 {
            let _ = SetStdHandle(STD_INPUT_HANDLE, in_handle);
        }
    }
}

#[cfg(not(windows))]
fn init_debug_console() {}

#[derive(Clone, Default)]
struct WindowState {
    relay: RelayConnectionState,
    quic: QuicConnectionState,
    remote_connection_telemetry: ConnectionTelemetryState,
    registry_connection_telemetry: ConnectionTelemetryState,
    shell_connection_telemetry: ConnectionTelemetryState,
    registry_relay: RegistryRelayConnectionState,
    registry_quic: RegistryQuicConnectionState,
    session_close: SessionCloseState,
    control: ControlState,
    registry_pending: RegistryPendingState,
    registry_control: RegistryControlState,
    remote_registry_pending: RemoteRegistryPendingState,
    file_transfer: FileTransferConnectionState,
    file_transfer_cancel: FileTransferCancelState,
    file_transfer_gate: FileTransferOperationGateState,
    shell: ShellConnectionState,
    shell_direct: ShellDirectConnectionState,
    shell_relay: ShellRelayConnectionState,
    shell_quic: ShellQuicConnectionState,
    chat: viewer_chat::ChatConnectionState,
    #[cfg(windows)]
    viewport: ViewportState,
    #[cfg(target_os = "macos")]
    viewport: MacViewportState,
}

impl WindowState {
    fn viewport_handle(&self) -> ViewportArc {
        #[cfg(any(windows, target_os = "macos"))]
        {
            self.viewport.inner.clone()
        }
        #[cfg(not(any(windows, target_os = "macos")))]
        {
            ViewportArc
        }
    }
}

#[derive(Clone, Default)]
struct AppWindowStates(pub Arc<Mutex<HashMap<String, WindowState>>>);

impl AppWindowStates {
    fn get_or_create(&self, label: &str) -> WindowState {
        let mut guard = self
            .0
            .lock()
            .expect("AppWindowStates mutex poisoned (get_or_create)");
        guard.entry(label.to_string()).or_default();
        guard
            .get(label)
            .cloned()
            .unwrap_or_else(WindowState::default)
    }

    fn remove(&self, label: &str) -> Option<WindowState> {
        let mut guard = self
            .0
            .lock()
            .expect("AppWindowStates mutex poisoned (remove)");
        guard.remove(label)
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ConnectionStatePayload {
    session_kind: String,
    transport: String,
    connection_type: String,
    encryption_label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    encryption_details: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    remote_addr: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    viewer_reflex: Option<ReflexAddress>,
    #[serde(skip_serializing_if = "Option::is_none")]
    agent_reflex: Option<ReflexAddress>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    agent_local_addrs: Vec<LocalAddr>,
    #[serde(skip_serializing_if = "Option::is_none")]
    connect_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    relay_tcp_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    relay_tls_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    relay_handshake_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    capture_type: Option<String>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct RemoteDesktopSnapshotPayload {
    image_base64: String,
    width: u32,
    height: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    mime_type: Option<String>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ConnectionStatsPayload {
    #[serde(flatten)]
    state: ConnectionStatePayload,
    sample_at_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    rtt_ms: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    avg_rtt_ms: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    min_rtt_ms: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_rtt_ms: Option<f64>,
    sample_count: u64,
}

#[derive(Deserialize)]
struct ConnectionPongMetaPayload {
    #[serde(rename = "type")]
    message_type: String,
    echoed_at_ms: u64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RemoteDesktopStreamMetaPayload {
    #[serde(default)]
    capture_type: Option<String>,
    #[serde(default)]
    capture_outputs: Vec<CaptureOutputInfoPayload>,
    #[serde(default)]
    active_index: Option<u32>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
struct CaptureOutputInfoPayload {
    index: u32,
    #[serde(default)]
    name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    display_id: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    width: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    height: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    origin_x: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    origin_y: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    point_width: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    point_height: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    primary: Option<bool>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
struct CaptureOutputsEventPayload {
    outputs: Vec<CaptureOutputInfoPayload>,
    active_index: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    capture_type: Option<String>,
}

#[derive(Default)]
struct ConnectionTelemetrySession {
    state: Option<ConnectionStatePayload>,
    sample_count: u64,
    total_rtt_ms: f64,
    last_rtt_ms: Option<f64>,
    min_rtt_ms: Option<f64>,
    max_rtt_ms: Option<f64>,
    recent_rtts: VecDeque<f64>,
    shutdown: Option<oneshot::Sender<()>>,
}

#[derive(Clone, Default)]
struct ConnectionTelemetryState(pub Arc<Mutex<ConnectionTelemetrySession>>);

impl ConnectionTelemetryState {
    fn replace_session(&self, state: ConnectionStatePayload) -> oneshot::Receiver<()> {
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        if let Ok(mut guard) = self.0.lock() {
            if let Some(existing) = guard.shutdown.take() {
                let _ = existing.send(());
            }
            *guard = ConnectionTelemetrySession {
                state: Some(state),
                shutdown: Some(shutdown_tx),
                ..ConnectionTelemetrySession::default()
            };
        }
        shutdown_rx
    }

    fn clear_transport(&self, transport: &str) {
        if let Ok(mut guard) = self.0.lock() {
            let matches = guard
                .state
                .as_ref()
                .map(|state| state.transport == transport)
                .unwrap_or(false);
            if matches {
                if let Some(existing) = guard.shutdown.take() {
                    let _ = existing.send(());
                }
                *guard = ConnectionTelemetrySession::default();
            }
        }
    }

    fn record_rtt(&self, rtt_ms: f64) {
        if let Ok(mut guard) = self.0.lock() {
            if guard.state.is_none() {
                return;
            }
            guard.last_rtt_ms = Some(rtt_ms);
            guard.sample_count = guard.sample_count.saturating_add(1);
            guard.total_rtt_ms += rtt_ms;
            guard.min_rtt_ms = Some(
                guard
                    .min_rtt_ms
                    .map(|current| current.min(rtt_ms))
                    .unwrap_or(rtt_ms),
            );
            guard.max_rtt_ms = Some(
                guard
                    .max_rtt_ms
                    .map(|current| current.max(rtt_ms))
                    .unwrap_or(rtt_ms),
            );
            guard.recent_rtts.push_back(rtt_ms);
            while guard.recent_rtts.len() > 120 {
                guard.recent_rtts.pop_front();
            }
        }
    }

    fn update_capture_type(&self, capture_type: &str) -> Option<ConnectionStatePayload> {
        let normalized = capture_type.trim();
        if normalized.is_empty() {
            return None;
        }
        let mut guard = self.0.lock().ok()?;
        let state = guard.state.as_mut()?;
        if state.capture_type.as_deref() == Some(normalized) {
            return None;
        }
        state.capture_type = Some(normalized.to_string());
        Some(state.clone())
    }

    fn snapshot(&self) -> Option<ConnectionStatsPayload> {
        let guard = self.0.lock().ok()?;
        let state = guard.state.clone()?;
        let avg_rtt_ms = if guard.sample_count == 0 {
            None
        } else {
            Some(guard.total_rtt_ms / guard.sample_count as f64)
        };
        Some(ConnectionStatsPayload {
            state,
            sample_at_ms: current_unix_ms(),
            rtt_ms: guard.last_rtt_ms,
            avg_rtt_ms,
            min_rtt_ms: guard.min_rtt_ms,
            max_rtt_ms: guard.max_rtt_ms,
            sample_count: guard.sample_count,
        })
    }
}

fn current_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u128::from(u64::MAX)) as u64
}

fn build_connection_ping_frame() -> Result<Vec<u8>, anyhow::Error> {
    build_control_frame(
        talos_protocol::CONTROL_TYPE_CONNECTION_PING,
        &current_unix_ms().to_be_bytes(),
    )
    .context("build connection ping frame")
}

fn emit_to_label<R, E, S>(emitter: &E, label: &str, event: &str, payload: S)
where
    R: tauri::Runtime,
    E: Emitter<R>,
    S: Serialize + Clone,
{
    if let Err(err) = emitter.emit_to(label, event, payload) {
        warn!(event = event, error = %err, "failed to emit window-scoped event");
    }
}

fn emit_window<R, S>(window: &Window<R>, event: &str, payload: S)
where
    R: tauri::Runtime,
    S: Serialize + Clone,
{
    emit_to_label(window, window.label(), event, payload);
}

fn emit_connection_state(window: &Window, payload: &ConnectionStatePayload) {
    emit_window(window, "connection:state", payload.clone());
}

fn emit_connection_stats(window: &Window, telemetry: &ConnectionTelemetryState) {
    let Some(snapshot) = telemetry.snapshot() else {
        return;
    };
    emit_window(window, "connection:stats", snapshot);
}

fn start_connection_telemetry(
    window: Window,
    telemetry: ConnectionTelemetryState,
    control_state: ControlState,
    payload: ConnectionStatePayload,
) {
    if payload.session_kind == "system_shell" {
        return;
    }

    let mut shutdown_rx = telemetry.replace_session(payload.clone());
    emit_connection_state(&window, &payload);
    emit_connection_stats(&window, &telemetry);
    tokio::spawn(async move {
        let mut ticker = interval(Duration::from_secs(1));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                _ = &mut shutdown_rx => break,
                _ = ticker.tick() => {
                    if let Some(sender) = control_state.sender() {
                        match build_connection_ping_frame() {
                            Ok(frame) => {
                                if sender.send(frame).is_err() {
                                    break;
                                }
                            }
                            Err(err) => {
                                warn!(error = %err, "failed to build connection ping frame");
                            }
                        }
                    }
                    emit_connection_stats(&window, &telemetry);
                }
            }
        }
    });
}

#[derive(Clone, Default)]
struct PendingSessionUrls(pub Arc<Mutex<HashMap<String, String>>>);

impl PendingSessionUrls {
    fn insert(&self, label: String, url: String) -> Result<(), String> {
        let mut guard = self
            .0
            .lock()
            .map_err(|_| "PendingSessionUrls mutex poisoned (insert)".to_string())?;
        guard.insert(label, url);
        Ok(())
    }

    fn take(&self, label: &str) -> Option<String> {
        self.0.lock().ok().and_then(|mut guard| guard.remove(label))
    }
}

fn next_session_window_label() -> String {
    let n = SESSION_WINDOW_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{SESSION_WINDOW_PREFIX}{n}")
}

fn launch_arg_url() -> Option<String> {
    std::env::args().find(|arg| arg.starts_with("rmm://"))
}

fn build_session_window(
    app: &AppHandle,
    pending_urls: &PendingSessionUrls,
    label: String,
) -> Result<(), String> {
    let result =
        WebviewWindowBuilder::new(app, label.clone(), WebviewUrl::App("index.html".into()))
            .title("Talos Viewer")
            .content_protected(false)
            .build();

    match result {
        Ok(session) => {
            apply_session_window_size(&session);
            disable_browser_accelerator_keys(&session);
            let _ = session.set_title("Talos Viewer Session");
            let _ = session.set_content_protected(false);
            allow_window_capture(&session);
            let _ = session.show();
            allow_window_capture(&session);
            activate_macos_app();
            let _ = session.set_focus();
            info!(label = %label, "session window created");
            Ok(())
        }
        Err(err) => {
            warn!(label = %label, error = %err, "failed to create session window");
            let _ = pending_urls.take(&label);
            Err(err.to_string())
        }
    }
}

fn queue_session_window(
    app: &AppHandle,
    pending_urls: &PendingSessionUrls,
    url: String,
) -> Result<String, String> {
    let url = url.trim().to_string();
    if !url.starts_with("rmm://") {
        return Err("queue_session_window expects an rmm:// URL".to_string());
    }

    let label = next_session_window_label();
    pending_urls.insert(label.clone(), url).map_err(|err| {
        warn!(error = %err, "failed to store pending session url");
        err
    })?;
    info!(%label, "spawning session window");

    // Creating windows is safest on the main thread/event loop.
    // Queue the creation and return the label immediately to avoid deadlocks.
    let app_for_build = app.clone();
    let label_for_cb = label.clone();
    let pending_urls_for_cb = pending_urls.clone();
    debug!(label = %label, "queueing session window on main thread");
    app.run_on_main_thread(move || {
        debug!(label = %label_for_cb, "running queued session window creation");
        let _ = build_session_window(&app_for_build, &pending_urls_for_cb, label_for_cb);
    })
    .map_err(|err| {
        let _ = pending_urls.take(&label);
        err.to_string()
    })?;

    Ok(label)
}

fn open_initial_session_window(
    app: &AppHandle,
    pending_urls: &PendingSessionUrls,
    url: String,
) -> Result<String, String> {
    let url = url.trim().to_string();
    if !url.starts_with("rmm://") {
        return Err("open_initial_session_window expects an rmm:// URL".to_string());
    }

    let label = next_session_window_label();
    pending_urls.insert(label.clone(), url).map_err(|err| {
        warn!(error = %err, "failed to store initial session url");
        err
    })?;
    build_session_window(app, pending_urls, label.clone())?;
    Ok(label)
}

#[tauri::command]
fn test_button() -> String {
    "Test button clicked".to_string()
}

#[tauri::command]
fn get_launch_args() -> Option<String> {
    launch_arg_url()
}

#[tauri::command]
fn get_arg_dump() -> Vec<String> {
    std::env::args().collect()
}

#[tauri::command]
fn get_window_label(window: Window) -> String {
    window.label().to_string()
}

#[tauri::command]
async fn spawn_session_window(
    window: Window,
    pending_urls: State<'_, PendingSessionUrls>,
    url: String,
) -> Result<String, String> {
    queue_session_window(window.app_handle(), &pending_urls, url)
}

#[tauri::command]
fn take_initial_url(window: Window, pending_urls: State<'_, PendingSessionUrls>) -> Option<String> {
    pending_urls.take(window.label())
}

#[tauri::command]
fn set_start_menu_blocked(blocked: bool) -> Result<(), String> {
    #[cfg(not(windows))]
    let _ = blocked;

    #[cfg(windows)]
    {
        unsafe { win_key_block_set_enabled(if blocked { 1 } else { 0 }) };
        if !blocked {
            force_release_forwarded_win_key("main.rs:set_start_menu_blocked");
        }
        #[cfg(debug_assertions)]
        debug!("start menu block set to {blocked}");
    }
    Ok(())
}

use talos_protocol::relay_transport::{
    build_e2e_cipher, build_relay_client_tls_config, parse_relay_target, read_e2e_frame_from,
    read_http_response, write_e2e_frame,
};
use talos_protocol::{
    build_control_frame, build_file_transfer_frame, parse_file_transfer_frame,
    FileTransferConflictMode, FileTransferEntry, FileTransferRequest, FileTransferResponse,
    LocalAddr, OperationErrorCode, ReflexAddress, RegistryHive, RegistryRequest, RegistryResponse,
    RegistryResponseEnvelope, RegistryValueData, RegistryValueEntry,
    CONTROL_PAYLOAD_CAPTURE_OUTPUT_INDEX_LEN, CONTROL_PAYLOAD_KEY_LEN,
    CONTROL_PAYLOAD_MOUSE_BUTTON_LEN, CONTROL_PAYLOAD_MOUSE_MOVE_LEN,
    CONTROL_PAYLOAD_MOUSE_WHEEL_LEN, CONTROL_PAYLOAD_SESSION_ID_LEN,
    CONTROL_TYPE_CAPTURE_OUTPUT_SWITCH, CONTROL_TYPE_CLIPBOARD, CONTROL_TYPE_KEY_DOWN,
    CONTROL_TYPE_KEY_UP, CONTROL_TYPE_MOUSE_BUTTON, CONTROL_TYPE_MOUSE_MOVE,
    CONTROL_TYPE_MOUSE_WHEEL, CONTROL_TYPE_REGISTRY_REQUEST, CONTROL_TYPE_SESSION_LOGOFF,
    CONTROL_TYPE_SESSION_SWITCH, CONTROL_TYPE_TYPED_INPUT, FILE_TRANSFER_DEFAULT_CHUNK_BYTES,
    FILE_TRANSFER_DEFAULT_ZIP_THRESHOLD_BYTES, FILE_TRANSFER_DEFAULT_ZIP_THRESHOLD_FILES,
    FILE_TRANSFER_MSG_DATA, FILE_TRANSFER_MSG_FINISH, FILE_TRANSFER_MSG_JSON,
    FILE_TRANSFER_STORE_ARCHIVE_THRESHOLD_BYTES, FILE_TRANSFER_STORE_ARCHIVE_THRESHOLD_FILES,
    HEARTBEAT_PAYLOAD, REGISTRY_META_MESSAGE_TYPE,
    REMOTE_DESKTOP_PROTOCOL_EXPERIMENTAL_DISPLAY_DELTA, REMOTE_DESKTOP_PROTOCOL_LEGACY_IVF,
    REMOTE_DESKTOP_PROTOCOL_MODERN_DISPLAY_DELTA,
};

#[cfg(any(windows, target_os = "macos"))]
use talos_protocol::{CONTROL_MOD_ALT, CONTROL_MOD_CTRL, CONTROL_MOD_SHIFT, CONTROL_MOD_WIN};

#[cfg(windows)]
#[derive(Clone, Default)]
pub(crate) struct ViewportState {
    inner: Arc<Mutex<ViewportInner>>,
}

#[cfg(windows)]
#[derive(Default)]
pub(crate) struct ViewportInner {
    child_hwnd: Option<HWND>,
    handle: Option<ViewportHandle>,
    context: Option<SoftbufferContext<ViewportDisplay>>,
    surface: Option<SoftbufferSurface<ViewportDisplay, ViewportHandle>>,
    last_size: Option<(u32, u32)>,
    last_rect: Option<(i32, i32, u32, u32)>,
    last_present: Option<Instant>,
    last_move_event: Option<Instant>,
    last_move_update: Option<Instant>,
    cached_frame: Option<CachedFrame>,
    last_cache_at: Option<Instant>,
    gpu_viewport: Option<viewport_d3d11::D3d11Viewport>,
    gpu_disabled: bool,
}

#[cfg(windows)]
unsafe impl Send for ViewportInner {}
#[cfg(windows)]
unsafe impl Sync for ViewportInner {}

#[cfg(windows)]
type ViewportArc = Arc<Mutex<ViewportInner>>;
#[cfg(target_os = "macos")]
type ViewportArc = Arc<Mutex<viewport_macos::ViewportInner>>;
#[cfg(all(not(windows), not(target_os = "macos")))]
#[derive(Clone)]
struct ViewportArc;

#[cfg(windows)]
#[derive(Clone, Copy)]
struct ViewportDisplay;

#[cfg(windows)]
impl HasDisplayHandle for ViewportDisplay {
    fn display_handle(&self) -> Result<DisplayHandle<'_>, HandleError> {
        let handle = WindowsDisplayHandle::new();
        unsafe { Ok(DisplayHandle::borrow_raw(RawDisplayHandle::Windows(handle))) }
    }
}

#[cfg(windows)]
#[derive(Clone, Copy)]
struct ViewportHandle {
    hwnd: HWND,
    hinstance: Option<NonZeroIsize>,
}

#[cfg(windows)]
impl HasWindowHandle for ViewportHandle {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        let hwnd = NonZeroIsize::new(self.hwnd as isize).ok_or(HandleError::Unavailable)?;
        let mut handle = Win32WindowHandle::new(hwnd);
        handle.hinstance = self.hinstance;
        unsafe { Ok(WindowHandle::borrow_raw(RawWindowHandle::Win32(handle))) }
    }
}

#[cfg(windows)]
static VIEWPORT_CLASS_REGISTERED: AtomicBool = AtomicBool::new(false);

#[cfg(windows)]
struct ViewportWmState {
    control_state: ControlState,
    focused: AtomicBool,
    last_mouse_move_ms: AtomicU64,
    forwarded_win_key_down: AtomicBool,
    forwarded_win_vkey: AtomicU16,
}

#[cfg(windows)]
static VIEWPORT_WM_STATES: OnceLock<Mutex<HashMap<isize, Arc<ViewportWmState>>>> = OnceLock::new();

#[cfg(windows)]
fn viewport_wm_states() -> &'static Mutex<HashMap<isize, Arc<ViewportWmState>>> {
    VIEWPORT_WM_STATES.get_or_init(|| Mutex::new(HashMap::new()))
}

#[cfg(windows)]
fn hwnd_key(hwnd: HWND) -> isize {
    hwnd as isize
}

#[cfg(windows)]
fn register_viewport_wm_state(hwnd: HWND, control_state: ControlState) {
    let state = Arc::new(ViewportWmState {
        control_state,
        focused: AtomicBool::new(false),
        last_mouse_move_ms: AtomicU64::new(0),
        forwarded_win_key_down: AtomicBool::new(false),
        forwarded_win_vkey: AtomicU16::new(VK_LWIN as u16),
    });
    if let Ok(mut guard) = viewport_wm_states().lock() {
        guard.insert(hwnd_key(hwnd), state);
    }
}

#[cfg(windows)]
fn unregister_viewport_wm_state(hwnd: HWND) {
    if let Ok(mut guard) = viewport_wm_states().lock() {
        guard.remove(&hwnd_key(hwnd));
    }
}

#[cfg(windows)]
fn get_viewport_wm_state(hwnd: HWND) -> Option<Arc<ViewportWmState>> {
    viewport_wm_states()
        .lock()
        .ok()
        .and_then(|guard| guard.get(&hwnd_key(hwnd)).cloned())
}
/// When set (e.g. RMM_VIEWER_DISABLE_INPUT_CAPTURE=1), viewport does not forward keyboard/mouse to remote (for testing Win key hook).
#[cfg(windows)]
static INPUT_CAPTURE_DISABLED: OnceLock<bool> = OnceLock::new();
#[cfg(windows)]
fn input_capture_disabled() -> bool {
    *INPUT_CAPTURE_DISABLED.get_or_init(|| {
        std::env::var_os("RMM_VIEWER_DISABLE_INPUT_CAPTURE")
            .is_some_and(|v| v == "1" || v == "true" || v == "TRUE")
    })
}

#[cfg(windows)]
fn force_release_forwarded_win_key(_location: &str) {
    let Some(map) = VIEWPORT_WM_STATES.get() else {
        return;
    };
    let states: Vec<Arc<ViewportWmState>> = match map.lock() {
        Ok(guard) => guard.values().cloned().collect(),
        Err(_) => return,
    };

    for state in states {
        if !state.forwarded_win_key_down.swap(false, Ordering::SeqCst) {
            continue;
        }
        let vkey = state.forwarded_win_vkey.load(Ordering::SeqCst);
        let Some(sender) = state.control_state.sender() else {
            continue;
        };
        let stream_size = state.control_state.stream_size();
        if let Ok((frame, _is_mouse_move)) = build_control_message(
            ControlEvent::KeyUp {
                vkey,
                scan: 0,
                modifiers: 0,
            },
            stream_size,
        ) {
            let _ = sender.send(frame);
        }
    }
}

#[cfg(not(windows))]
fn force_release_forwarded_win_key(_location: &str) {}

#[cfg(windows)]
const CF_UNICODETEXT: u32 = 13;
#[cfg(windows)]
const VK_CONTROL: i32 = 0x11;
#[cfg(windows)]
const VK_MENU: i32 = 0x12;
#[cfg(windows)]
const VK_SHIFT: i32 = 0x10;
#[cfg(windows)]
const VK_LWIN: i32 = 0x5B;
#[cfg(windows)]
const VK_RWIN: i32 = 0x5C;

#[cfg(windows)]
extern "system" {
    fn ScreenToClient(hwnd: HWND, point: *mut POINT) -> i32;
    fn SetFocus(hwnd: HWND) -> HWND;
    fn OpenClipboard(hwnd: HWND) -> i32;
    fn CloseClipboard() -> i32;
    fn GetClipboardData(format: u32) -> *mut std::ffi::c_void;
    fn GetKeyState(n_virt_key: i32) -> i16;
}

#[cfg(windows)]
fn viewport_recently_moved(
    viewport: &Arc<Mutex<ViewportInner>>,
    threshold_ms: u128,
) -> Option<u128> {
    let now = Instant::now();
    let Ok(guard) = viewport.lock() else {
        return None;
    };
    let last = guard.last_move_event?;
    let since = now.duration_since(last).as_millis();
    if since < threshold_ms {
        Some(since)
    } else {
        None
    }
}

#[cfg(windows)]
fn to_wide(input: &str) -> Vec<u16> {
    std::ffi::OsStr::new(input)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

#[cfg(windows)]
fn ensure_viewport_class(hinstance: HWND) -> Result<Vec<u16>, String> {
    let class_name = to_wide("RmmViewport");
    if VIEWPORT_CLASS_REGISTERED.load(Ordering::SeqCst) {
        return Ok(class_name);
    }

    let wnd_class = WNDCLASSW {
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: Some(viewport_wndproc),
        hInstance: hinstance,
        lpszClassName: class_name.as_ptr(),
        ..unsafe { std::mem::zeroed() }
    };

    unsafe {
        RegisterClassW(&wnd_class);
    }
    VIEWPORT_CLASS_REGISTERED.store(true, Ordering::SeqCst);
    Ok(class_name)
}

#[cfg(windows)]
fn move_child_on_window_event(
    viewport: &Arc<Mutex<ViewportInner>>,
    window: &Window,
    event: &WindowEvent,
) {
    match event {
        WindowEvent::Moved(_) | WindowEvent::Resized(_) => {}
        _ => return,
    }
    let Ok(mut guard) = viewport.lock() else {
        return;
    };
    let Some(child_hwnd) = guard.child_hwnd else {
        return;
    };
    let Some((x, y, width, height)) = guard.last_rect else {
        return;
    };
    let now = Instant::now();
    guard.last_move_event = Some(now);
    if let Some(last) = guard.last_move_update {
        if now.duration_since(last) < Duration::from_millis(8) {
            return;
        }
    }
    guard.last_move_update = Some(now);
    let tauri_hwnd = match window.window_handle().ok().map(|h| h.as_raw()) {
        Some(RawWindowHandle::Win32(handle)) => handle.hwnd.get() as HWND,
        _ => return,
    };
    let mut pt = POINT { x, y };
    let converted = unsafe { ClientToScreen(tauri_hwnd, &mut pt) != 0 };
    if !converted {
        return;
    }
    unsafe {
        SetWindowPos(
            child_hwnd,
            HWND_TOP,
            pt.x,
            pt.y,
            width as i32,
            height as i32,
            SWP_NOACTIVATE | SWP_SHOWWINDOW,
        );
    }
    let _ = present_cached_frame(&mut guard);
}

#[cfg(windows)]
fn create_child_viewport(
    parent: HWND,
) -> Result<
    (
        HWND,
        ViewportHandle,
        SoftbufferContext<ViewportDisplay>,
        SoftbufferSurface<ViewportDisplay, ViewportHandle>,
    ),
    String,
> {
    let hinstance = unsafe { GetModuleHandleW(std::ptr::null()) };
    let class_name = ensure_viewport_class(hinstance)?;

    let hwnd = unsafe {
        CreateWindowExW(
            0,
            class_name.as_ptr(),
            class_name.as_ptr(),
            WS_POPUP | WS_VISIBLE,
            0,
            0,
            1,
            1,
            parent,
            std::ptr::null_mut(),
            hinstance,
            std::ptr::null(),
        )
    };
    if hwnd.is_null() {
        return Err("failed to create viewport window".to_string());
    }

    let hinstance_ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_HINSTANCE) };
    let handle = ViewportHandle {
        hwnd,
        hinstance: NonZeroIsize::new(hinstance_ptr),
    };
    let display = ViewportDisplay;
    let context = SoftbufferContext::new(display)
        .map_err(|err| format!("softbuffer context error: {err}"))?;
    let surface = SoftbufferSurface::new(&context, handle)
        .map_err(|err| format!("softbuffer surface error: {err}"))?;

    Ok((hwnd, handle, context, surface))
}

#[cfg(windows)]
fn update_viewport(
    window: &Window,
    state: &mut ViewportInner,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    occlusions: &[ViewportOcclusionRect],
) -> Result<(), String> {
    if state.child_hwnd.is_none() {
        let raw = window
            .window_handle()
            .map_err(|err| err.to_string())?
            .as_raw();
        let tauri_hwnd = match raw {
            RawWindowHandle::Win32(handle) => handle.hwnd.get() as HWND,
            _ => return Err("unsupported window handle".to_string()),
        };
        let (hwnd, handle, context, surface) = create_child_viewport(tauri_hwnd)?;
        state.child_hwnd = Some(hwnd);
        state.handle = Some(handle);
        state.context = Some(context);
        state.surface = Some(surface);
        state.last_size = None;
    }

    let Some(hwnd) = state.child_hwnd else {
        return Err("viewport window missing".to_string());
    };

    let tauri_hwnd = window
        .window_handle()
        .map_err(|err| err.to_string())
        .and_then(|h| match h.as_raw() {
            RawWindowHandle::Win32(handle) => Ok(handle.hwnd.get() as HWND),
            _ => Err("unsupported window handle".to_string()),
        })?;
    let mut client_rect = RECT {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    let got_rect = unsafe { GetClientRect(tauri_hwnd, &mut client_rect) != 0 };
    let (width, height) = if got_rect {
        let client_w = (client_rect.right - client_rect.left).max(0);
        let client_h = (client_rect.bottom - client_rect.top).max(0);
        let max_w = (client_w - x).max(0) as u32;
        let max_h = (client_h - y).max(0) as u32;
        (width.min(max_w), height.min(max_h))
    } else {
        (width, height)
    };

    state.last_rect = Some((x, y, width, height));
    let mut pt = POINT { x, y };
    let _ = unsafe { ClientToScreen(tauri_hwnd, &mut pt) != 0 };
    unsafe {
        SetWindowPos(
            hwnd,
            HWND_TOP,
            pt.x,
            pt.y,
            width as i32,
            height as i32,
            SWP_NOACTIVATE | SWP_SHOWWINDOW,
        );
    }
    apply_viewport_clip_region(hwnd, x, y, width, height, occlusions)?;

    if state.surface.is_none() {
        return Err("viewport surface missing".to_string());
    }

    // Repaint the cached last-visible frame immediately on geometry updates so the user
    // sees responsive resizes even while the decoder is temporarily skipping presents.
    // Best-effort: if it fails, the next decoded frame will still present.
    let _ = present_cached_frame(state);

    Ok(())
}

#[cfg(windows)]
unsafe extern "system" fn viewport_wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_SETFOCUS => {
            if let Some(state) = get_viewport_wm_state(hwnd) {
                state.focused.store(true, Ordering::SeqCst);
            }

            // Register RIDEV_NOHOTKEYS to suppress Win key at the Raw Input
            // layer.  This is independent of the LL hook and effective even
            // if the hook callback times out on the first keypress.
            let _nohotkeys_ret = unsafe { win_key_register_nohotkeys(hwnd) };

            #[cfg(debug_assertions)]
            debug!("viewport focused");
        }
        WM_KILLFOCUS => {
            if let Some(state) = get_viewport_wm_state(hwnd) {
                state.focused.store(false, Ordering::SeqCst);
            }

            // Deregister RIDEV_NOHOTKEYS so system hotkeys work normally
            // when the viewport is not focused.
            let _nohotkeys_ret = unsafe { win_key_deregister_nohotkeys() };

            force_release_forwarded_win_key("main.rs:viewport_wndproc:WM_KILLFOCUS");
            #[cfg(debug_assertions)]
            debug!("viewport lost focus");
        }
        WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN => {
            SetFocus(hwnd);
            if let Some(state) = get_viewport_wm_state(hwnd) {
                state.focused.store(true, Ordering::SeqCst);
            }
            #[cfg(debug_assertions)]
            debug!("viewport mouse down");
            if input_capture_disabled() {
                return DefWindowProcW(hwnd, msg, wparam, lparam);
            }
            let Some(state) = get_viewport_wm_state(hwnd) else {
                return DefWindowProcW(hwnd, msg, wparam, lparam);
            };
            if let Some(event) = build_mouse_button_event(hwnd, msg, lparam) {
                send_native_control_with_state(&state, event);
            }
        }
        WM_LBUTTONUP | WM_RBUTTONUP | WM_MBUTTONUP => {
            if input_capture_disabled() {
                return DefWindowProcW(hwnd, msg, wparam, lparam);
            }
            let Some(state) = get_viewport_wm_state(hwnd) else {
                return DefWindowProcW(hwnd, msg, wparam, lparam);
            };
            if let Some(event) = build_mouse_button_event(hwnd, msg, lparam) {
                send_native_control_with_state(&state, event);
            }
        }
        WM_MOUSEMOVE => {
            let Some(state) = get_viewport_wm_state(hwnd) else {
                return DefWindowProcW(hwnd, msg, wparam, lparam);
            };
            if !state.focused.load(Ordering::SeqCst) {
                return DefWindowProcW(hwnd, msg, wparam, lparam);
            }
            if should_throttle_mouse_move(&state) {
                return DefWindowProcW(hwnd, msg, wparam, lparam);
            }
            if !input_capture_disabled() {
                if let Some(event) = build_mouse_move_event(hwnd, lparam) {
                    send_native_control_with_state(&state, event);
                }
            }
        }
        WM_MOUSEWHEEL => {
            let Some(state) = get_viewport_wm_state(hwnd) else {
                return DefWindowProcW(hwnd, msg, wparam, lparam);
            };
            if !state.focused.load(Ordering::SeqCst) {
                return DefWindowProcW(hwnd, msg, wparam, lparam);
            }
            if !input_capture_disabled() {
                if let Some(event) = build_mouse_wheel_event(hwnd, wparam, lparam) {
                    send_native_control_with_state(&state, event);
                }
            }
        }
        WM_KEYDOWN | WM_SYSKEYDOWN | WM_KEYUP | WM_SYSKEYUP => {
            let vkey = wparam as u16;
            let Some(state) = get_viewport_wm_state(hwnd) else {
                return DefWindowProcW(hwnd, msg, wparam, lparam);
            };
            if !state.focused.load(Ordering::SeqCst) {
                return DefWindowProcW(hwnd, msg, wparam, lparam);
            }
            if !input_capture_disabled() {
                if let Some(event) = build_key_event(Some(state.as_ref()), msg, wparam, lparam) {
                    send_native_control_with_state(&state, event);
                }
            }
            // Do NOT pass Win key messages to DefWindowProcW — its default
            // handling triggers the Start menu.  Return 0 to swallow them.
            if vkey == VK_LWIN as u16 || vkey == VK_RWIN as u16 {
                return 0;
            }
        }
        WM_NCDESTROY => {
            unregister_viewport_wm_state(hwnd);
        }
        _ => {}
    }
    DefWindowProcW(hwnd, msg, wparam, lparam)
}

#[cfg(windows)]
fn should_throttle_mouse_move(state: &ViewportWmState) -> bool {
    let now = unsafe { GetTickCount64() };
    let last = state.last_mouse_move_ms.load(Ordering::Relaxed);
    if now.saturating_sub(last) < 8 {
        true
    } else {
        state.last_mouse_move_ms.store(now, Ordering::Relaxed);
        false
    }
}

#[cfg(windows)]
fn build_mouse_move_event(hwnd: HWND, lparam: LPARAM) -> Option<ControlEvent> {
    let (x, y, width, height) = viewport_client_metrics_from_lparam(hwnd, lparam)?;
    Some(ControlEvent::MouseMove {
        x,
        y,
        element_width: width,
        element_height: height,
    })
}

#[cfg(windows)]
fn build_mouse_button_event(hwnd: HWND, msg: u32, lparam: LPARAM) -> Option<ControlEvent> {
    let (x, y, width, height) = viewport_client_metrics_from_lparam(hwnd, lparam)?;
    let (button, down) = match msg {
        WM_LBUTTONDOWN => (0, true),
        WM_LBUTTONUP => (0, false),
        WM_RBUTTONDOWN => (1, true),
        WM_RBUTTONUP => (1, false),
        WM_MBUTTONDOWN => (2, true),
        WM_MBUTTONUP => (2, false),
        _ => return None,
    };
    Some(ControlEvent::MouseButton {
        button,
        down,
        x,
        y,
        element_width: width,
        element_height: height,
    })
}

#[cfg(windows)]
fn build_mouse_wheel_event(hwnd: HWND, wparam: WPARAM, lparam: LPARAM) -> Option<ControlEvent> {
    let delta = ((wparam >> 16) & 0xFFFF) as i16;
    if delta == 0 {
        return None;
    }
    let mut pt = POINT {
        x: (lparam & 0xFFFF) as i16 as i32,
        y: ((lparam >> 16) & 0xFFFF) as i16 as i32,
    };
    unsafe {
        if ScreenToClient(hwnd, &mut pt) == 0 {
            return None;
        }
    }
    let (width, height) = viewport_client_size(hwnd)?;
    let x = pt.x.clamp(0, width.saturating_sub(1) as i32) as u32;
    let y = pt.y.clamp(0, height.saturating_sub(1) as i32) as u32;
    Some(ControlEvent::MouseWheel {
        delta,
        x,
        y,
        element_width: width,
        element_height: height,
    })
}

#[cfg(windows)]
fn build_key_event(
    state: Option<&ViewportWmState>,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> Option<ControlEvent> {
    let is_down = matches!(msg, WM_KEYDOWN | WM_SYSKEYDOWN);
    let vkey = wparam as u16;
    if vkey == VK_LWIN as u16 || vkey == VK_RWIN as u16 {
        // When RIDEV_NOHOTKEYS is active the LL hook never fires for Win keys,
        // so we forward from the viewport path instead.  Track the forwarded
        // state so force_release_forwarded_win_key() can clean up.
        if let Some(state) = state {
            if is_down {
                state.forwarded_win_vkey.store(vkey, Ordering::SeqCst);
                state.forwarded_win_key_down.store(true, Ordering::SeqCst);
            } else {
                state.forwarded_win_key_down.store(false, Ordering::SeqCst);
            }
        }
        // Send with scan=0 so the agent injects via wVk (not KEYEVENTF_SCANCODE).
        // The Win key is an extended key whose scan code (0x5B with E0 prefix)
        // requires KEYEVENTF_EXTENDEDKEY which the agent's build_key_input
        // does not set.  Using wVk=VK_LWIN/VK_RWIN avoids this entirely.
        //
        // Also strip CONTROL_MOD_WIN from modifiers — current_modifiers()
        // reports VK_LWIN as held while it's being pressed, which would cause
        // the agent's push_modifier_inputs to inject a *second* VK_LWIN down.
        // On key-up the modifier bit might already be clear, so the agent would
        // never release the first injection → sticky Win key on the remote.
        let modifiers = current_modifiers() & !CONTROL_MOD_WIN;
        if is_down {
            return Some(ControlEvent::KeyDown {
                vkey,
                scan: 0,
                modifiers,
            });
        } else {
            return Some(ControlEvent::KeyUp {
                vkey,
                scan: 0,
                modifiers,
            });
        }
    }
    let scan = ((lparam >> 16) & 0xFF) as u16;
    let modifiers = current_modifiers();
    if is_ctrl_shift_f(vkey, modifiers) && is_down {
        if let Some(text) = read_clipboard_text() {
            return Some(ControlEvent::TypedInput { text });
        }
    }
    if is_down {
        Some(ControlEvent::KeyDown {
            vkey,
            scan,
            modifiers,
        })
    } else {
        Some(ControlEvent::KeyUp {
            vkey,
            scan,
            modifiers,
        })
    }
}

#[cfg(windows)]
fn is_ctrl_shift_f(vkey: u16, modifiers: u8) -> bool {
    vkey == 0x46 && (modifiers & CONTROL_MOD_CTRL != 0) && (modifiers & CONTROL_MOD_SHIFT != 0)
}

#[cfg(windows)]
fn current_modifiers() -> u8 {
    let mut mods = 0u8;
    unsafe {
        if (GetKeyState(VK_CONTROL) as u16 & 0x8000) != 0 {
            mods |= CONTROL_MOD_CTRL;
        }
        if (GetKeyState(VK_SHIFT) as u16 & 0x8000) != 0 {
            mods |= CONTROL_MOD_SHIFT;
        }
        if (GetKeyState(VK_MENU) as u16 & 0x8000) != 0 {
            mods |= CONTROL_MOD_ALT;
        }
        if (GetKeyState(VK_LWIN) as u16 & 0x8000) != 0
            || (GetKeyState(VK_RWIN) as u16 & 0x8000) != 0
        {
            mods |= CONTROL_MOD_WIN;
        }
    }
    mods
}

#[cfg(windows)]
fn read_clipboard_text() -> Option<String> {
    unsafe {
        if OpenClipboard(std::ptr::null_mut()) == 0 {
            return None;
        }
        let handle = GetClipboardData(CF_UNICODETEXT);
        if handle.is_null() {
            CloseClipboard();
            return None;
        }
        let ptr = GlobalLock(handle) as *const u16;
        if ptr.is_null() {
            CloseClipboard();
            return None;
        }
        let mut len = 0usize;
        while *ptr.add(len) != 0 {
            len += 1;
        }
        let slice = std::slice::from_raw_parts(ptr, len);
        let text = String::from_utf16_lossy(slice);
        GlobalUnlock(handle);
        CloseClipboard();
        Some(text)
    }
}

#[cfg(windows)]
fn viewport_client_metrics_from_lparam(hwnd: HWND, lparam: LPARAM) -> Option<(u32, u32, u32, u32)> {
    let x = (lparam & 0xFFFF) as i16 as i32;
    let y = ((lparam >> 16) & 0xFFFF) as i16 as i32;
    let (width, height) = viewport_client_size(hwnd)?;
    let clamped_x = x.clamp(0, width.saturating_sub(1) as i32) as u32;
    let clamped_y = y.clamp(0, height.saturating_sub(1) as i32) as u32;
    Some((clamped_x, clamped_y, width, height))
}

#[cfg(windows)]
fn viewport_client_size(hwnd: HWND) -> Option<(u32, u32)> {
    let mut rect = RECT {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    let ok = unsafe { GetClientRect(hwnd, &mut rect) != 0 };
    if !ok {
        return None;
    }
    let width = (rect.right - rect.left).max(0) as u32;
    let height = (rect.bottom - rect.top).max(0) as u32;
    Some((width, height))
}

#[cfg(windows)]
fn send_native_control_with_state(state: &ViewportWmState, event: ControlEvent) {
    send_native_control_with_control_state(&state.control_state, event);
}

#[cfg(any(windows, target_os = "macos"))]
pub(crate) fn send_native_control_with_control_state(
    control_state: &ControlState,
    event: ControlEvent,
) {
    let sender = match control_state.sender() {
        Some(sender) => sender,
        None => {
            if should_log_control_event(&event) {
                warn!(
                    detail = %control_event_detail(&event),
                    "viewer native control send skipped: no active control sender"
                );
            }
            return;
        }
    };
    let stream_size = control_state.stream_size();
    let event_detail = control_event_detail(&event);
    let event_kind = control_event_kind(&event);
    let should_log = should_log_control_event(&event);
    match build_control_message(event, stream_size) {
        Ok((frame, is_mouse_move)) => {
            if sender.send(frame).is_err() {
                if is_mouse_move {
                    debug!("viewer native mouse-move control send failed: control channel closed");
                } else {
                    warn!(
                        kind = event_kind,
                        detail = %event_detail,
                        "viewer native control send failed: control channel closed"
                    );
                }
            }
        }
        Err(err) => {
            if should_log {
                warn!(
                    kind = event_kind,
                    detail = %event_detail,
                    error = %err,
                    "viewer native control frame build failed"
                );
            }
        }
    }
}

pub(crate) fn viewer_local_addrs() -> Vec<(Ipv4Addr, u8)> {
    let mut addrs = Vec::new();
    if let Ok(interfaces) = get_if_addrs() {
        for iface in interfaces {
            match iface.addr {
                IfAddr::V4(v4) => {
                    if v4.is_loopback() || v4.ip.is_link_local() {
                        continue;
                    }
                    let prefix = netmask_to_prefix(v4.netmask);
                    addrs.push((v4.ip, prefix));
                }
                IfAddr::V6(_) => {}
            }
        }
    }
    addrs
}

fn netmask_to_prefix(mask: Ipv4Addr) -> u8 {
    mask.octets()
        .iter()
        .map(|octet| octet.count_ones())
        .sum::<u32>() as u8
}

fn network_id(ip: Ipv4Addr, prefix: u8) -> u32 {
    let mask = if prefix == 0 {
        0
    } else {
        u32::MAX << (32 - prefix)
    };
    u32::from(ip) & mask
}

pub(crate) fn pick_lan_candidate(
    viewer_addrs: &[(Ipv4Addr, u8)],
    agent_addrs: &[LocalAddr],
) -> Option<String> {
    let mut matches: Vec<(u8, String)> = Vec::new();
    for (viewer_ip, viewer_prefix) in viewer_addrs {
        for agent in agent_addrs {
            if let Ok(agent_ip) = agent.ip.parse::<Ipv4Addr>() {
                let prefix = (*viewer_prefix).min(agent.prefix);
                if network_id(*viewer_ip, prefix) == network_id(agent_ip, prefix) {
                    matches.push((prefix, agent.ip.clone()));
                }
            }
        }
    }
    matches.sort_by_key(|(prefix, _)| *prefix);
    matches.first().map(|(_, ip)| ip.clone())
}

pub(crate) fn query_configured_stun_reflex(stun_socket: UdpSocket) -> anyhow::Result<SocketAddr> {
    let stun_server = talos_protocol::configured_stun_server()
        .context("validate RMM_STUN_SERVER")?
        .context(
            "STUN is disabled; set RMM_STUN_SERVER to opt in to direct public-UDP discovery",
        )?;
    let stun_addr = stun_server
        .to_socket_addrs()
        .with_context(|| format!("resolve configured STUN server {stun_server}"))?
        .find(SocketAddr::is_ipv4)
        .context("configured STUN server did not resolve to an IPv4 address")?;
    let mut client = StunClient::new(stun_addr);
    client
        .set_timeout(Duration::from_secs(2))
        .set_retry_interval(Duration::from_millis(250));
    client
        .query_external_address(&stun_socket)
        .context("query configured STUN server")
}

#[tauri::command]
fn get_viewer_transport() -> String {
    let value = std::env::var("RMM_VIEWER_TRANSPORT").unwrap_or_else(|_| "auto".to_string());
    let value = value.trim().to_lowercase();
    if value == "quic" || value == "tcprelay" {
        value
    } else {
        "auto".to_string()
    }
}

#[cfg(any(windows, target_os = "macos"))]
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ViewportOcclusionRect {
    x: i32,
    y: i32,
    width: u32,
    height: u32,
}

// Linux renders through the webview and accepts, but does not apply, native occlusions.
#[cfg(not(any(windows, target_os = "macos")))]
#[derive(Debug, Clone, Deserialize)]
struct ViewportOcclusionRect {
    #[serde(rename = "x")]
    _x: i32,
    #[serde(rename = "y")]
    _y: i32,
    #[serde(rename = "width")]
    _width: u32,
    #[serde(rename = "height")]
    _height: u32,
}

#[cfg(windows)]
fn apply_viewport_clip_region(
    hwnd: HWND,
    viewport_x: i32,
    viewport_y: i32,
    viewport_width: u32,
    viewport_height: u32,
    occlusions: &[ViewportOcclusionRect],
) -> Result<(), String> {
    if viewport_width == 0 || viewport_height == 0 {
        return Ok(());
    }

    let width_i32 =
        i32::try_from(viewport_width).map_err(|_| "viewport width exceeds i32".to_string())?;
    let height_i32 =
        i32::try_from(viewport_height).map_err(|_| "viewport height exceeds i32".to_string())?;

    unsafe {
        let base_rgn = CreateRectRgn(0, 0, width_i32, height_i32);
        if base_rgn.is_null() {
            return Err("CreateRectRgn(base) failed".to_string());
        }

        for occlusion in occlusions {
            if occlusion.width == 0 || occlusion.height == 0 {
                continue;
            }

            let occ_w = match i32::try_from(occlusion.width) {
                Ok(v) => v,
                Err(_) => continue,
            };
            let occ_h = match i32::try_from(occlusion.height) {
                Ok(v) => v,
                Err(_) => continue,
            };

            // Convert absolute window-client occlusion rect into viewport-local coordinates.
            let mut left = occlusion.x.saturating_sub(viewport_x).saturating_sub(1);
            let mut top = occlusion.y.saturating_sub(viewport_y).saturating_sub(1);
            let mut right = left.saturating_add(occ_w).saturating_add(2);
            let mut bottom = top.saturating_add(occ_h).saturating_add(2);

            // Clamp to viewport bounds before subtracting.
            left = left.clamp(0, width_i32);
            top = top.clamp(0, height_i32);
            right = right.clamp(0, width_i32);
            bottom = bottom.clamp(0, height_i32);
            if right <= left || bottom <= top {
                continue;
            }

            let occ_rgn = CreateRectRgn(left, top, right, bottom);
            if occ_rgn.is_null() {
                continue;
            }
            let _ = CombineRgn(base_rgn, base_rgn, occ_rgn, RGN_DIFF);
            let _ = DeleteObject(occ_rgn);
        }

        // On success, ownership transfers to the window; do not delete base_rgn.
        let ok = SetWindowRgn(hwnd, base_rgn, 1);
        if ok == 0 {
            let _ = DeleteObject(base_rgn);
            return Err("SetWindowRgn failed".to_string());
        }
    }

    Ok(())
}

fn encode_argb_png_base64(width: u32, height: u32, argb: &[u32]) -> Result<String, String> {
    let expected_pixels = width as usize * height as usize;
    if width == 0 || height == 0 || argb.len() != expected_pixels {
        return Err("remote desktop snapshot frame is invalid".to_string());
    }

    let mut rgba = Vec::with_capacity(expected_pixels * 4);
    for pixel in argb {
        rgba.push(((pixel >> 16) & 0xFF) as u8);
        rgba.push(((pixel >> 8) & 0xFF) as u8);
        rgba.push((pixel & 0xFF) as u8);
        rgba.push(((pixel >> 24) & 0xFF) as u8);
    }

    let mut png_bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut png_bytes, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder
            .write_header()
            .map_err(|err| format!("snapshot png header failed: {err}"))?;
        writer
            .write_image_data(&rgba)
            .map_err(|err| format!("snapshot png encode failed: {err}"))?;
    }

    Ok(BASE64_STANDARD.encode(png_bytes))
}

#[cfg(any(not(windows), test))]
fn encode_argb_bmp_base64(width: u32, height: u32, argb: &[u32]) -> Result<String, String> {
    let expected_pixels = width as usize * height as usize;
    if width == 0 || height == 0 || argb.len() != expected_pixels {
        return Err("remote desktop frame is invalid".to_string());
    }

    let row_stride = (width as usize)
        .checked_mul(4)
        .ok_or_else(|| "remote desktop frame is too large".to_string())?;
    let pixel_bytes = row_stride
        .checked_mul(height as usize)
        .ok_or_else(|| "remote desktop frame is too large".to_string())?;
    let file_size = 14usize
        .checked_add(40)
        .and_then(|header| header.checked_add(pixel_bytes))
        .ok_or_else(|| "remote desktop frame is too large".to_string())?;
    let file_size_u32 =
        u32::try_from(file_size).map_err(|_| "remote desktop frame is too large".to_string())?;
    let pixel_bytes_u32 =
        u32::try_from(pixel_bytes).map_err(|_| "remote desktop frame is too large".to_string())?;

    let mut bmp = Vec::with_capacity(file_size);
    bmp.extend_from_slice(b"BM");
    bmp.extend_from_slice(&file_size_u32.to_le_bytes());
    bmp.extend_from_slice(&0u16.to_le_bytes());
    bmp.extend_from_slice(&0u16.to_le_bytes());
    bmp.extend_from_slice(&54u32.to_le_bytes());
    bmp.extend_from_slice(&40u32.to_le_bytes());
    bmp.extend_from_slice(&(width as i32).to_le_bytes());
    bmp.extend_from_slice(&(-(height as i32)).to_le_bytes());
    bmp.extend_from_slice(&1u16.to_le_bytes());
    bmp.extend_from_slice(&32u16.to_le_bytes());
    bmp.extend_from_slice(&0u32.to_le_bytes());
    bmp.extend_from_slice(&pixel_bytes_u32.to_le_bytes());
    bmp.extend_from_slice(&0i32.to_le_bytes());
    bmp.extend_from_slice(&0i32.to_le_bytes());
    bmp.extend_from_slice(&0u32.to_le_bytes());
    bmp.extend_from_slice(&0u32.to_le_bytes());

    for pixel in argb {
        bmp.push((pixel & 0xFF) as u8);
        bmp.push(((pixel >> 8) & 0xFF) as u8);
        bmp.push(((pixel >> 16) & 0xFF) as u8);
        bmp.push(((pixel >> 24) & 0xFF) as u8);
    }

    Ok(BASE64_STANDARD.encode(bmp))
}

#[cfg(not(windows))]
fn cache_nonwindows_remote_desktop_frame(window: &Window, frame: &DecodedFrame) {
    cache_nonwindows_remote_desktop_frame_for_label(window.label(), frame);
}

#[cfg(not(windows))]
fn cache_nonwindows_remote_desktop_frame_for_label(label: &str, frame: &DecodedFrame) {
    let cache = NON_WINDOWS_REMOTE_DESKTOP_FRAMES.get_or_init(|| Mutex::new(HashMap::new()));
    if let Ok(mut guard) = cache.lock() {
        guard.insert(
            label.to_string(),
            NonWindowsCachedFrame {
                width: frame.width,
                height: frame.height,
                argb: frame.argb.clone(),
            },
        );
    }
}

#[cfg(not(windows))]
fn emit_remote_desktop_frame(window: &Window, frame: &DecodedFrame) -> Result<(), String> {
    cache_nonwindows_remote_desktop_frame(window, frame);
    let image_base64 = encode_argb_bmp_base64(frame.width, frame.height, &frame.argb)?;
    if let Ok(path) = std::env::var("TALOS_VIEWER_DUMP_FRAME") {
        if !path.trim().is_empty() {
            if let Ok(bytes) = BASE64_STANDARD.decode(image_base64.as_bytes()) {
                let _ = fs::write(&path, bytes);
            }
        }
    }
    emit_window(
        window,
        "remote-desktop:frame",
        RemoteDesktopSnapshotPayload {
            image_base64,
            width: frame.width,
            height: frame.height,
            mime_type: Some("image/bmp".to_string()),
        },
    );
    Ok(())
}

#[cfg(target_os = "macos")]
fn present_or_emit_remote_desktop_frame(
    window: &Window,
    viewport: &ViewportArc,
    frame: &DecodedFrame,
) -> Result<(), String> {
    cache_nonwindows_remote_desktop_frame(window, frame);
    if !viewport_macos::native_viewport_enabled() {
        return emit_remote_desktop_frame(window, frame);
    }

    if objc2::MainThreadMarker::new().is_some() {
        let present_result = viewport
            .lock()
            .map_err(|err| err.to_string())
            .and_then(|mut guard| guard.present_decoded_frame(frame.clone()));
        return match present_result {
            Ok(true) => Ok(()),
            Ok(false) => emit_remote_desktop_frame(window, frame),
            Err(err) => {
                let should_warn = viewport
                    .lock()
                    .map(|mut guard| guard.take_fallback_warning())
                    .unwrap_or(true);
                if should_warn {
                    warn!(
                        error = %err,
                        "macOS native viewport failed; falling back to webview BMP frame events"
                    );
                }
                emit_remote_desktop_frame(window, frame)
            }
        };
    }

    let frame_for_present = frame.clone();
    let viewport_for_present = viewport.clone();
    let (tx, rx) = std::sync::mpsc::channel();
    let schedule_result = window.run_on_main_thread(move || {
        let result = viewport_for_present
            .lock()
            .map_err(|err| err.to_string())
            .and_then(|mut guard| guard.present_decoded_frame(frame_for_present));
        let _ = tx.send(result);
    });

    let present_result = match schedule_result {
        Ok(()) => rx
            .recv_timeout(Duration::from_secs(1))
            .map_err(|err| err.to_string())?,
        Err(err) => Err(err.to_string()),
    };

    match present_result {
        Ok(true) => Ok(()),
        Ok(false) => emit_remote_desktop_frame(window, frame),
        Err(err) => {
            let should_warn = viewport
                .lock()
                .map(|mut guard| guard.take_fallback_warning())
                .unwrap_or(true);
            if should_warn {
                warn!(
                    error = %err,
                    "macOS native viewport failed; falling back to webview BMP frame events"
                );
            }
            emit_remote_desktop_frame(window, frame)
        }
    }
}

#[cfg(all(not(windows), not(target_os = "macos")))]
fn present_or_emit_remote_desktop_frame(
    window: &Window,
    _viewport: &ViewportArc,
    frame: &DecodedFrame,
) -> Result<(), String> {
    emit_remote_desktop_frame(window, frame)
}

#[cfg(windows)]
#[tauri::command]
fn capture_remote_desktop_snapshot(
    window: Window,
    window_states: State<'_, AppWindowStates>,
) -> Result<RemoteDesktopSnapshotPayload, String> {
    let state = window_states.get_or_create(window.label());
    let (width, height, argb) = {
        let mut guard = state.viewport.inner.lock().map_err(|err| err.to_string())?;
        let experimental_frame = if let Some(viewport) = guard.gpu_viewport.as_ref() {
            viewport.read_experimental_desktop_argb()?
        } else {
            None
        };
        if let Some((width, height, argb)) = experimental_frame {
            guard.cached_frame = Some(CachedFrame {
                width,
                height,
                argb: argb.clone(),
            });
            (width, height, argb)
        } else {
            let frame = guard
                .cached_frame
                .as_ref()
                .ok_or_else(|| "No remote desktop frame is available yet.".to_string())?;
            (frame.width, frame.height, frame.argb.clone())
        }
    };

    let image_base64 = encode_argb_png_base64(width, height, &argb)?;
    Ok(RemoteDesktopSnapshotPayload {
        image_base64,
        width,
        height,
        mime_type: Some("image/png".to_string()),
    })
}

#[cfg(not(windows))]
#[tauri::command]
fn capture_remote_desktop_snapshot(
    window: Window,
    _window_states: State<'_, AppWindowStates>,
) -> Result<RemoteDesktopSnapshotPayload, String> {
    let frame = NON_WINDOWS_REMOTE_DESKTOP_FRAMES
        .get()
        .and_then(|cache| {
            cache
                .lock()
                .ok()
                .and_then(|guard| guard.get(window.label()).cloned())
        })
        .ok_or_else(|| "No remote desktop frame is available yet.".to_string())?;
    let image_base64 = encode_argb_png_base64(frame.width, frame.height, &frame.argb)?;
    Ok(RemoteDesktopSnapshotPayload {
        image_base64,
        width: frame.width,
        height: frame.height,
        mime_type: Some("image/png".to_string()),
    })
}

#[cfg(windows)]
#[tauri::command]
fn viewport_set_rect(
    window: Window,
    window_states: State<'_, AppWindowStates>,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    occlusions: Option<Vec<ViewportOcclusionRect>>,
) -> Result<(), String> {
    let state = window_states.get_or_create(window.label());
    let inner = state.viewport.inner.clone();
    let control_state = state.control.clone();
    let scale = window.scale_factor().unwrap_or(1.0);
    let scaled_x = (x as f64 * scale).round() as i32;
    let scaled_y = (y as f64 * scale).round() as i32;
    let scaled_width = (width as f64 * scale).round() as u32;
    let scaled_height = (height as f64 * scale).round() as u32;
    let scaled_occlusions: Vec<ViewportOcclusionRect> = occlusions
        .unwrap_or_default()
        .into_iter()
        .map(|rect| ViewportOcclusionRect {
            x: (rect.x as f64 * scale).round() as i32,
            y: (rect.y as f64 * scale).round() as i32,
            width: ((rect.width as f64) * scale).round().max(0.0) as u32,
            height: ((rect.height as f64) * scale).round().max(0.0) as u32,
        })
        .collect();
    let window_clone = window.clone();
    window
        .run_on_main_thread(move || {
            let mut guard = inner.lock().expect("viewport lock poisoned");
            if scaled_width == 0 || scaled_height == 0 {
                if let Some(hwnd) = guard.child_hwnd {
                    unsafe {
                        ShowWindow(hwnd, SW_HIDE);
                    }
                }
                guard.last_size = None;
                guard.last_rect = None;
                return;
            }

            if let Err(err) = update_viewport(
                &window_clone,
                &mut guard,
                scaled_x,
                scaled_y,
                scaled_width,
                scaled_height,
                &scaled_occlusions,
            ) {
                warn!(error = %err, "viewport update failed");
            }

            // Ensure the native viewport wndproc can route input to the correct session.
            if let Some(hwnd) = guard.child_hwnd {
                if get_viewport_wm_state(hwnd).is_none() {
                    register_viewport_wm_state(hwnd, control_state.clone());
                }
            }
        })
        .map_err(|err| err.to_string())?;

    Ok(())
}

#[cfg(target_os = "macos")]
#[tauri::command]
fn viewport_set_rect(
    window: Window,
    window_states: State<'_, AppWindowStates>,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    occlusions: Option<Vec<ViewportOcclusionRect>>,
) -> Result<(), String> {
    let state = window_states.get_or_create(window.label());
    let inner = state.viewport.inner.clone();
    let control_state = state.control.clone();
    let scale = window.scale_factor().unwrap_or(1.0);
    let occlusions = occlusions.unwrap_or_default();
    let window_clone = window.clone();
    window
        .run_on_main_thread(move || {
            let mut guard = match inner.lock() {
                Ok(guard) => guard,
                Err(err) => {
                    warn!(error = %err, "macOS native viewport lock poisoned");
                    return;
                }
            };
            if let Err(err) = guard.update_rect(
                &window_clone,
                control_state,
                x,
                y,
                width,
                height,
                &occlusions,
                scale,
            ) {
                let should_warn = guard.take_fallback_warning();
                if should_warn {
                    warn!(
                        error = %err,
                        "macOS native viewport update failed; falling back to webview BMP frame events"
                    );
                }
            }
        })
        .map_err(|err| err.to_string())?;

    Ok(())
}

#[cfg(all(not(windows), not(target_os = "macos")))]
#[tauri::command]
fn viewport_set_rect(
    _window: Window,
    _window_states: State<'_, AppWindowStates>,
    _x: i32,
    _y: i32,
    _width: u32,
    _height: u32,
    _occlusions: Option<Vec<ViewportOcclusionRect>>,
) -> Result<(), String> {
    Ok(())
}

#[derive(Debug, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub(crate) enum ControlEvent {
    MouseMove {
        x: u32,
        y: u32,
        element_width: u32,
        element_height: u32,
    },
    MouseButton {
        button: u8,
        down: bool,
        x: u32,
        y: u32,
        element_width: u32,
        element_height: u32,
    },
    MouseWheel {
        delta: i16,
        x: u32,
        y: u32,
        element_width: u32,
        element_height: u32,
    },
    KeyDown {
        vkey: u16,
        scan: u16,
        modifiers: u8,
    },
    KeyUp {
        vkey: u16,
        scan: u16,
        modifiers: u8,
    },
    Clipboard {
        text: String,
    },
    TypedInput {
        text: String,
    },
    SessionSwitch {
        session_id: u32,
    },
    SessionLogoff {
        session_id: u32,
    },
    CaptureOutputSwitch {
        index: u32,
    },
}

fn control_event_kind(event: &ControlEvent) -> &'static str {
    match event {
        ControlEvent::MouseMove { .. } => "mouse_move",
        ControlEvent::MouseButton { .. } => "mouse_button",
        ControlEvent::MouseWheel { .. } => "mouse_wheel",
        ControlEvent::KeyDown { .. } => "key_down",
        ControlEvent::KeyUp { .. } => "key_up",
        ControlEvent::Clipboard { .. } => "clipboard",
        ControlEvent::TypedInput { .. } => "typed_input",
        ControlEvent::SessionSwitch { .. } => "session_switch",
        ControlEvent::SessionLogoff { .. } => "session_logoff",
        ControlEvent::CaptureOutputSwitch { .. } => "capture_output_switch",
    }
}

fn control_event_detail(event: &ControlEvent) -> String {
    match event {
        ControlEvent::MouseMove {
            x,
            y,
            element_width,
            element_height,
        } => format!("kind=mouse_move x={x} y={y} element={element_width}x{element_height}"),
        ControlEvent::MouseButton {
            button,
            down,
            x,
            y,
            element_width,
            element_height,
        } => format!(
            "kind=mouse_button button={button} down={down} x={x} y={y} element={element_width}x{element_height}"
        ),
        ControlEvent::MouseWheel {
            delta,
            x,
            y,
            element_width,
            element_height,
        } => format!(
            "kind=mouse_wheel delta={delta} x={x} y={y} element={element_width}x{element_height}"
        ),
        ControlEvent::KeyDown {
            vkey,
            scan,
            modifiers,
        } => format!("kind=key_down vkey={vkey} scan={scan} modifiers={modifiers}"),
        ControlEvent::KeyUp {
            vkey,
            scan,
            modifiers,
        } => format!("kind=key_up vkey={vkey} scan={scan} modifiers={modifiers}"),
        ControlEvent::Clipboard { text } => {
            format!("kind=clipboard bytes={}", text.len())
        }
        ControlEvent::TypedInput { text } => {
            format!("kind=typed_input chars={}", text.chars().count())
        }
        ControlEvent::SessionSwitch { session_id } => {
            format!("kind=session_switch session_id={session_id}")
        }
        ControlEvent::SessionLogoff { session_id } => {
            format!("kind=session_logoff session_id={session_id}")
        }
        ControlEvent::CaptureOutputSwitch { index } => {
            format!("kind=capture_output_switch index={index}")
        }
    }
}

fn should_log_control_event(event: &ControlEvent) -> bool {
    !matches!(event, ControlEvent::MouseMove { .. })
}

#[tauri::command]
async fn send_control(
    window: Window,
    window_states: State<'_, AppWindowStates>,
    event: ControlEvent,
) -> Result<(), String> {
    let state = window_states.get_or_create(window.label());
    let sender = match state.control.sender() {
        Some(sender) => sender,
        None => {
            if should_log_control_event(&event) {
                warn!(
                    detail = %control_event_detail(&event),
                    "viewer control send skipped: no active control sender"
                );
            }
            return Ok(());
        }
    };
    let stream_size = state.control.stream_size();
    let event_detail = control_event_detail(&event);
    let event_kind = control_event_kind(&event);
    let should_log = should_log_control_event(&event);
    let (frame, is_mouse_move) = match build_control_message(event, stream_size) {
        Ok(frame) => frame,
        Err(err) => {
            if should_log {
                warn!(kind = event_kind, detail = %event_detail, error = %err, "viewer control frame build failed");
            }
            return Err(err);
        }
    };
    if is_mouse_move {
        if sender.send(frame).is_err() {
            debug!("viewer mouse-move control send failed: control channel closed");
        }
        return Ok(());
    }
    sender
        .send(frame)
        .map_err(|_| {
            warn!(kind = event_kind, detail = %event_detail, "viewer control send failed: control channel closed");
            "control channel closed".to_string()
        })
}

fn build_control_message(
    event: ControlEvent,
    stream_size: Option<(u32, u32)>,
) -> Result<(Vec<u8>, bool), String> {
    match event {
        ControlEvent::MouseMove {
            x,
            y,
            element_width,
            element_height,
        } => {
            let (nx, ny) = normalize_coords(x, y, element_width, element_height, stream_size);
            let mut payload = Vec::with_capacity(CONTROL_PAYLOAD_MOUSE_MOVE_LEN);
            payload.extend_from_slice(&nx.to_be_bytes());
            payload.extend_from_slice(&ny.to_be_bytes());
            build_control_frame(CONTROL_TYPE_MOUSE_MOVE, &payload)
                .map(|frame| (frame, true))
                .map_err(|err| err.to_string())
        }
        ControlEvent::MouseButton {
            button,
            down,
            x,
            y,
            element_width,
            element_height,
        } => {
            let (nx, ny) = normalize_coords(x, y, element_width, element_height, stream_size);
            let mut payload = Vec::with_capacity(CONTROL_PAYLOAD_MOUSE_BUTTON_LEN);
            payload.push(button);
            payload.push(if down { 1 } else { 0 });
            payload.extend_from_slice(&nx.to_be_bytes());
            payload.extend_from_slice(&ny.to_be_bytes());
            build_control_frame(CONTROL_TYPE_MOUSE_BUTTON, &payload)
                .map(|frame| (frame, false))
                .map_err(|err| err.to_string())
        }
        ControlEvent::MouseWheel {
            delta,
            x,
            y,
            element_width,
            element_height,
        } => {
            let (nx, ny) = normalize_coords(x, y, element_width, element_height, stream_size);
            let mut payload = Vec::with_capacity(CONTROL_PAYLOAD_MOUSE_WHEEL_LEN);
            payload.extend_from_slice(&delta.to_be_bytes());
            payload.extend_from_slice(&nx.to_be_bytes());
            payload.extend_from_slice(&ny.to_be_bytes());
            build_control_frame(CONTROL_TYPE_MOUSE_WHEEL, &payload)
                .map(|frame| (frame, false))
                .map_err(|err| err.to_string())
        }
        ControlEvent::KeyDown {
            vkey,
            scan,
            modifiers,
        } => build_key_frame(CONTROL_TYPE_KEY_DOWN, vkey, scan, modifiers),
        ControlEvent::KeyUp {
            vkey,
            scan,
            modifiers,
        } => build_key_frame(CONTROL_TYPE_KEY_UP, vkey, scan, modifiers),
        ControlEvent::Clipboard { text } => build_text_frame(CONTROL_TYPE_CLIPBOARD, &text),
        ControlEvent::TypedInput { text } => build_text_frame(CONTROL_TYPE_TYPED_INPUT, &text),
        ControlEvent::SessionSwitch { session_id } => {
            let mut payload = Vec::with_capacity(CONTROL_PAYLOAD_SESSION_ID_LEN);
            payload.extend_from_slice(&session_id.to_be_bytes());
            build_control_frame(CONTROL_TYPE_SESSION_SWITCH, &payload)
                .map(|frame| (frame, false))
                .map_err(|err| err.to_string())
        }
        ControlEvent::SessionLogoff { session_id } => {
            let mut payload = Vec::with_capacity(CONTROL_PAYLOAD_SESSION_ID_LEN);
            payload.extend_from_slice(&session_id.to_be_bytes());
            build_control_frame(CONTROL_TYPE_SESSION_LOGOFF, &payload)
                .map(|frame| (frame, false))
                .map_err(|err| err.to_string())
        }
        ControlEvent::CaptureOutputSwitch { index } => {
            let mut payload = Vec::with_capacity(CONTROL_PAYLOAD_CAPTURE_OUTPUT_INDEX_LEN);
            payload.extend_from_slice(&index.to_be_bytes());
            build_control_frame(CONTROL_TYPE_CAPTURE_OUTPUT_SWITCH, &payload)
                .map(|frame| (frame, false))
                .map_err(|err| err.to_string())
        }
    }
}

fn build_key_frame(
    message_type: u8,
    vkey: u16,
    scan: u16,
    modifiers: u8,
) -> Result<(Vec<u8>, bool), String> {
    let mut payload = Vec::with_capacity(CONTROL_PAYLOAD_KEY_LEN);
    payload.extend_from_slice(&vkey.to_be_bytes());
    payload.extend_from_slice(&scan.to_be_bytes());
    payload.push(modifiers);
    build_control_frame(message_type, &payload)
        .map(|frame| (frame, false))
        .map_err(|err| err.to_string())
}

fn build_text_frame(message_type: u8, text: &str) -> Result<(Vec<u8>, bool), String> {
    let bytes = text.as_bytes();
    if bytes.len() > u16::MAX as usize {
        return Err("control text payload too large".to_string());
    }
    let mut payload = Vec::with_capacity(2 + bytes.len());
    payload.extend_from_slice(&(bytes.len() as u16).to_be_bytes());
    payload.extend_from_slice(bytes);
    build_control_frame(message_type, &payload)
        .map(|frame| (frame, false))
        .map_err(|err| err.to_string())
}

fn normalize_coords(
    x: u32,
    y: u32,
    element_width: u32,
    element_height: u32,
    stream_size: Option<(u32, u32)>,
) -> (u32, u32) {
    if element_width == 0 || element_height == 0 {
        return (0, 0);
    }
    let element_w = element_width as f64;
    let element_h = element_height as f64;
    let (frame_w, frame_h, offset_x, offset_y) = match stream_size {
        Some((stream_w, stream_h)) if stream_w > 0 && stream_h > 0 => {
            let stream_w = stream_w as f64;
            let stream_h = stream_h as f64;
            let stream_aspect = stream_w / stream_h;
            let element_aspect = element_w / element_h;
            if stream_aspect > element_aspect {
                let frame_w = element_w;
                let frame_h = (element_w / stream_aspect).max(1.0);
                (frame_w, frame_h, 0.0, (element_h - frame_h) / 2.0)
            } else {
                let frame_h = element_h;
                let frame_w = (element_h * stream_aspect).max(1.0);
                (frame_w, frame_h, (element_w - frame_w) / 2.0, 0.0)
            }
        }
        _ => (element_w.max(1.0), element_h.max(1.0), 0.0, 0.0),
    };
    let local_x = (x as f64 - offset_x).clamp(0.0, frame_w);
    let local_y = (y as f64 - offset_y).clamp(0.0, frame_h);
    let nx = ((local_x / frame_w) * 65535.0).round().clamp(0.0, 65535.0) as u32;
    let ny = ((local_y / frame_h) * 65535.0).round().clamp(0.0, 65535.0) as u32;
    (nx, ny)
}

#[tauri::command]
async fn connect_quic(
    window: Window,
    window_states: State<'_, AppWindowStates>,
    session_id: String,
    token: String,
    agent_reflex: ReflexAddress,
    agent_host: Option<String>,
    agent_local_addrs: Option<Vec<LocalAddr>>,
    psk_cert_pem: String,
    api_base: Option<String>,
    quic_timeout_ms: Option<u64>,
    selected_stream_protocol: Option<String>,
) -> Result<(), String> {
    let state = window_states.get_or_create(window.label());
    let quic_state = state.quic.clone();
    let control_state = state.control.clone();
    let telemetry_state = state.remote_connection_telemetry.clone();
    let registry_pending = state.registry_pending.clone();
    let viewport = state.viewport_handle();

    set_session_close_context(
        &state.session_close,
        session_id.clone(),
        token.clone(),
        api_base.clone(),
    );

    if let Some(existing) = quic_state.0.lock().map_err(|e| e.to_string())?.take() {
        let _ = existing.shutdown.send(());
        existing.handle.abort();
    }

    let viewer_addrs = viewer_local_addrs();
    // Only use LAN when we have a positive subnet match. Do NOT fall back to agent_host when
    // agent_local_addrs exists but no match—agent_host may be a private IP (e.g. Azure 172.16.x)
    // unreachable from the viewer's network.
    let lan_candidate = match &agent_local_addrs {
        Some(addrs) => pick_lan_candidate(&viewer_addrs, addrs),
        None => agent_host.clone().filter(|h| !h.trim().is_empty()),
    };
    let reflex_addr: SocketAddr = format!("{}:{}", agent_reflex.ip, agent_reflex.port)
        .parse()
        .map_err(|e: std::net::AddrParseError| e.to_string())?;
    let lan_addr = lan_candidate
        .as_ref()
        .map(|ip| format!("{}:{}", ip, agent_reflex.port).parse::<SocketAddr>())
        .transpose()
        .map_err(|e: std::net::AddrParseError| e.to_string())?;
    let agent_local_addrs_log = agent_local_addrs.as_ref().map(|addrs| {
        addrs
            .iter()
            .map(|addr| format!("{}/{}", addr.ip, addr.prefix))
            .collect::<Vec<_>>()
    });
    info!(
        session_id = %session_id,
        lan_candidate = ?lan_candidate,
        target = %reflex_addr,
        "connect_quic invoked"
    );
    debug!(
        session_id = %session_id,
        viewer_addrs = ?viewer_addrs,
        agent_local_addrs = ?agent_local_addrs_log,
        agent_host = ?agent_host,
        "viewer transport candidates resolved"
    );
    let window_handle = window.clone();
    let (shutdown_tx, mut shutdown_rx) = oneshot::channel();
    let window_for_shutdown = window_handle.clone();
    let control_for_shutdown = control_state.clone();
    let telemetry_for_shutdown = telemetry_state.clone();
    let session_for_shutdown = session_id.clone();
    let stream_protocol =
        selected_stream_protocol.unwrap_or_else(|| REMOTE_DESKTOP_PROTOCOL_LEGACY_IVF.to_string());
    let task = tokio::spawn(async move {
        tokio::select! {
            _ = &mut shutdown_rx => {
                info!(session_id = %session_for_shutdown, "quic connection shutdown requested");
                control_for_shutdown.clear();
                telemetry_for_shutdown.clear_transport("quic");
                emit_window(&window_for_shutdown, "quic:ended", ());
            }
            _ = async move {
        let mut last_err: Option<anyhow::Error> = None;

        let Some(api_base) = api_base.clone() else {
            last_err = Some(anyhow!("missing api base"));
            if let Some(err) = last_err {
                warn!(error = %err, "quic connection failed");
                emit_window(&window_handle, "quic:error", err.to_string());
            }
            return;
        };

        let socket = match UdpSocket::bind("0.0.0.0:0") {
            Ok(s) => s,
            Err(err) => {
                last_err = Some(anyhow!(err.to_string()));
                if let Some(err) = last_err {
                    warn!(error = %err, "quic connection failed");
                    emit_window(&window_handle, "quic:error", err.to_string());
                }
                return;
            }
        };
        if let Err(err) = socket.set_nonblocking(true) {
            last_err = Some(anyhow!(err.to_string()));
            if let Some(err) = last_err {
                warn!(error = %err, "quic connection failed");
                emit_window(&window_handle, "quic:error", err.to_string());
            }
            return;
        }

        let viewer_reflex = match tokio::task::spawn_blocking({
            let stun_socket = socket.try_clone().ok();
            move || -> Result<SocketAddr, anyhow::Error> {
                let stun_socket = stun_socket.ok_or_else(|| anyhow!("stun socket clone failed"))?;
                query_configured_stun_reflex(stun_socket)
            }
        })
        .await
        {
            Ok(Ok(addr)) => addr,
            Ok(Err(err)) => {
                last_err = Some(err);
                if let Some(err) = last_err {
                    warn!(error = %err, "quic connection failed");
                    emit_window(&window_handle, "quic:error", err.to_string());
                }
                return;
            }
            Err(err) => {
                last_err = Some(anyhow!(err.to_string()));
                if let Some(err) = last_err {
                    warn!(error = %err, "quic connection failed");
                    emit_window(&window_handle, "quic:error", err.to_string());
                }
                return;
            }
        };

        let url = format!(
            "{}/api/rmm/session/{}/viewer-reflex?token={}",
            api_base,
            session_id,
            urlencoding::encode(&token)
        );
        let body = serde_json::json!({
            "ip": viewer_reflex.ip().to_string(),
            "port": viewer_reflex.port()
        });
        if let Err(err) = Client::new().post(url).json(&body).send().await {
            last_err = Some(anyhow!(err.to_string()));
            if let Some(err) = last_err {
                warn!(error = %err, "quic connection failed");
                emit_window(&window_handle, "quic:error", err.to_string());
            }
            return;
        }

        let mut endpoint = match Endpoint::new(
            EndpointConfig::default(),
            None,
            socket,
            Arc::new(TokioRuntime),
        ) {
            Ok(ep) => ep,
            Err(err) => {
                last_err = Some(anyhow!(err.to_string()));
                if let Some(err) = last_err {
                    warn!(error = %err, "quic connection failed");
                    emit_window(&window_handle, "quic:error", err.to_string());
                }
                return;
            }
        };
        let client_config = match build_client_config(&psk_cert_pem) {
            Ok(cfg) => cfg,
            Err(err) => {
                last_err = Some(err);
                if let Some(err) = last_err {
                    warn!(error = %err, "quic connection failed");
                    emit_window(&window_handle, "quic:error", err.to_string());
                }
                return;
            }
        };
        endpoint.set_default_client_config(client_config);

        // Registry reconnect often needs a longer window right after disconnect.
        let quic_timeout = Duration::from_millis(quic_timeout_ms.unwrap_or(2000));
        if let Some(lan_addr) = lan_addr {
            let lan_started_at = Instant::now();
            let mut lan_handle = tokio::spawn(run_quic_with_timeout(
                endpoint.clone(),
                session_id.clone(),
                lan_addr,
                quic_timeout,
            ));
            let reflex_started_at = Instant::now();
            let mut reflex_handle = tokio::spawn(run_quic_with_timeout(
                endpoint.clone(),
                session_id.clone(),
                reflex_addr,
                quic_timeout,
            ));

            let mut lan_done = false;
            let mut reflex_done = false;
            loop {
                tokio::select! {
                    result = &mut lan_handle, if !lan_done => {
                        match result {
                            Ok(Ok(connection)) => {
                                reflex_handle.abort();
                                info!(session_id = %session_id, source = "lan", target = %lan_addr, "quic connected");
                                let (control_tx, control_rx) = mpsc::unbounded_channel();
                                control_state.set_sender(Some(control_tx));
                                control_state.set_stream_size(None);
                                start_connection_telemetry(
                                    window_handle.clone(),
                                    telemetry_state.clone(),
                                    control_state.clone(),
                                    ConnectionStatePayload {
                                        session_kind: "remote_desktop".to_string(),
                                        transport: "quic".to_string(),
                                        connection_type: "lan_direct".to_string(),
                                        encryption_label: "Pinned QUIC TLS".to_string(),
                                        encryption_details: Some(
                                            "Direct QUIC session authenticated with the per-session pinned certificate.".to_string()
                                        ),
                                        remote_addr: Some(connection.remote_address().to_string()),
                                        viewer_reflex: Some(ReflexAddress {
                                            ip: viewer_reflex.ip().to_string(),
                                            port: viewer_reflex.port(),
                                        }),
                                        agent_reflex: Some(agent_reflex.clone()),
                                        agent_local_addrs: agent_local_addrs.clone().unwrap_or_default(),
                                        connect_ms: Some(lan_started_at.elapsed().as_millis().min(u128::from(u64::MAX)) as u64),
                                        relay_tcp_ms: None,
                                        relay_tls_ms: None,
                                        relay_handshake_ms: None,
                                        capture_type: None,
                                    }
                                );
                                match run_quic_stream(
                                    window_handle.clone(),
                                    session_id.clone(),
                                    connection,
                                    control_state.clone(),
                                    registry_pending.clone(),
                                    telemetry_state.clone(),
                                    viewport.clone(),
                                    control_rx,
                                    stream_protocol.clone(),
                                )
                                .await
                                {
                                    Ok(()) => return,
                                    Err(err) => {
                                        telemetry_state.clear_transport("quic");
                                        last_err = Some(err);
                                    }
                                }
                                break;
                            }
                            Ok(Err(err)) => {
                                debug!(session_id = %session_id, source = "lan", target = %lan_addr, error = %err, "quic candidate failed");
                                last_err = Some(err);
                                lan_done = true;
                            }
                            Err(err) => {
                                debug!(session_id = %session_id, source = "lan", target = %lan_addr, error = %err, "quic candidate task failed");
                                last_err = Some(anyhow!(err.to_string()));
                                lan_done = true;
                            }
                        }
                    }
                    result = &mut reflex_handle, if !reflex_done => {
                        match result {
                            Ok(Ok(connection)) => {
                                lan_handle.abort();
                                info!(session_id = %session_id, source = "reflex", target = %reflex_addr, "quic connected");
                                let (control_tx, control_rx) = mpsc::unbounded_channel();
                                control_state.set_sender(Some(control_tx));
                                control_state.set_stream_size(None);
                                start_connection_telemetry(
                                    window_handle.clone(),
                                    telemetry_state.clone(),
                                    control_state.clone(),
                                    ConnectionStatePayload {
                                        session_kind: "remote_desktop".to_string(),
                                        transport: "quic".to_string(),
                                        connection_type: "hole_punch".to_string(),
                                        encryption_label: "Pinned QUIC TLS".to_string(),
                                        encryption_details: Some(
                                            "Peer-to-peer QUIC session authenticated with the per-session pinned certificate.".to_string()
                                        ),
                                        remote_addr: Some(connection.remote_address().to_string()),
                                        viewer_reflex: Some(ReflexAddress {
                                            ip: viewer_reflex.ip().to_string(),
                                            port: viewer_reflex.port(),
                                        }),
                                        agent_reflex: Some(agent_reflex.clone()),
                                        agent_local_addrs: agent_local_addrs.clone().unwrap_or_default(),
                                        connect_ms: Some(reflex_started_at.elapsed().as_millis().min(u128::from(u64::MAX)) as u64),
                                        relay_tcp_ms: None,
                                        relay_tls_ms: None,
                                        relay_handshake_ms: None,
                                        capture_type: None,
                                    }
                                );
                                match run_quic_stream(
                                    window_handle.clone(),
                                    session_id.clone(),
                                    connection,
                                    control_state.clone(),
                                    registry_pending.clone(),
                                    telemetry_state.clone(),
                                    viewport.clone(),
                                    control_rx,
                                    stream_protocol.clone(),
                                )
                                .await
                                {
                                    Ok(()) => return,
                                    Err(err) => {
                                        telemetry_state.clear_transport("quic");
                                        last_err = Some(err);
                                    }
                                }
                                break;
                            }
                            Ok(Err(err)) => {
                                debug!(session_id = %session_id, source = "reflex", target = %reflex_addr, error = %err, "quic candidate failed");
                                last_err = Some(err);
                                reflex_done = true;
                            }
                            Err(err) => {
                                debug!(session_id = %session_id, source = "reflex", target = %reflex_addr, error = %err, "quic candidate task failed");
                                last_err = Some(anyhow!(err.to_string()));
                                reflex_done = true;
                            }
                        }
                    }
                    else => break,
                }
            }
        } else {
            let quic_timeout = Duration::from_millis(quic_timeout_ms.unwrap_or(500));
            let reflex_started_at = Instant::now();
            match run_quic_with_timeout(
                endpoint.clone(),
                session_id.clone(),
                reflex_addr,
                quic_timeout,
            )
            .await
            {
                Ok(connection) => {
                    info!(session_id = %session_id, source = "reflex", target = %reflex_addr, "quic connected");
                    let (control_tx, control_rx) = mpsc::unbounded_channel();
                    control_state.set_sender(Some(control_tx));
                    control_state.set_stream_size(None);
                    start_connection_telemetry(
                        window_handle.clone(),
                        telemetry_state.clone(),
                        control_state.clone(),
                        ConnectionStatePayload {
                            session_kind: "remote_desktop".to_string(),
                            transport: "quic".to_string(),
                            connection_type: "hole_punch".to_string(),
                            encryption_label: "Pinned QUIC TLS".to_string(),
                            encryption_details: Some(
                                "Peer-to-peer QUIC session authenticated with the per-session pinned certificate.".to_string()
                            ),
                            remote_addr: Some(connection.remote_address().to_string()),
                            viewer_reflex: Some(ReflexAddress {
                                ip: viewer_reflex.ip().to_string(),
                                port: viewer_reflex.port(),
                            }),
                            agent_reflex: Some(agent_reflex.clone()),
                            agent_local_addrs: agent_local_addrs.clone().unwrap_or_default(),
                            connect_ms: Some(reflex_started_at.elapsed().as_millis().min(u128::from(u64::MAX)) as u64),
                            relay_tcp_ms: None,
                            relay_tls_ms: None,
                            relay_handshake_ms: None,
                            capture_type: None,
                        }
                    );
                    match run_quic_stream(
                            window_handle.clone(),
                        session_id.clone(),
                        connection,
                        control_state.clone(),
                        registry_pending.clone(),
                        telemetry_state.clone(),
                        viewport.clone(),
                        control_rx,
                        stream_protocol.clone(),
                    )
                    .await
                    {
                        Ok(()) => return,
                        Err(err) => {
                            telemetry_state.clear_transport("quic");
                            last_err = Some(err);
                        }
                    }
                }
                Err(err) => {
                    debug!(session_id = %session_id, source = "reflex", target = %reflex_addr, error = %err, "quic candidate failed");
                    last_err = Some(err);
                }
            }
        }

        if let Some(err) = last_err {
            telemetry_state.clear_transport("quic");
            warn!(error = %err, "quic connection failed");
            emit_window(&window_handle, "quic:error", err.to_string());
        }
            } => {}
        }
    });
    quic_state
        .0
        .lock()
        .map_err(|e| e.to_string())?
        .replace(QuicConnectionTask {
            handle: task,
            shutdown: shutdown_tx,
        });
    Ok(())
}

#[tauri::command]
async fn registry_connect_quic(
    window: Window,
    window_states: State<'_, AppWindowStates>,
    session_id: String,
    token: String,
    agent_reflex: ReflexAddress,
    agent_host: Option<String>,
    agent_local_addrs: Option<Vec<LocalAddr>>,
    psk_cert_pem: String,
    api_base: Option<String>,
    quic_timeout_ms: Option<u64>,
) -> Result<(), String> {
    let state = window_states.get_or_create(window.label());
    let quic_state = state.registry_quic.clone();
    let control_state = state.registry_control.clone();
    let pending_state = state.remote_registry_pending.clone();
    let telemetry_state = state.registry_connection_telemetry.clone();

    if let Some(existing) = quic_state.0.lock().map_err(|e| e.to_string())?.take() {
        let _ = existing.shutdown.send(());
        existing.handle.abort();
    }

    pending_state.0.lock().await.clear();
    control_state.clear();

    let viewer_addrs = viewer_local_addrs();
    let lan_candidate = match &agent_local_addrs {
        Some(addrs) => pick_lan_candidate(&viewer_addrs, addrs),
        None => agent_host.clone().filter(|h| !h.trim().is_empty()),
    };
    let reflex_addr: SocketAddr = format!("{}:{}", agent_reflex.ip, agent_reflex.port)
        .parse()
        .map_err(|e: std::net::AddrParseError| e.to_string())?;
    let lan_addr = lan_candidate
        .as_ref()
        .map(|ip| format!("{}:{}", ip, agent_reflex.port).parse::<SocketAddr>())
        .transpose()
        .map_err(|e: std::net::AddrParseError| e.to_string())?;
    info!(
        session_id = %session_id,
        lan_candidate = ?lan_candidate,
        target = %reflex_addr,
        "registry_connect_quic invoked"
    );

    let window_handle = window.clone();
    let (shutdown_tx, mut shutdown_rx) = oneshot::channel();
    let window_for_shutdown = window_handle.clone();
    let control_for_shutdown = control_state.clone();
    let pending_for_shutdown = pending_state.clone();
    let session_for_shutdown = session_id.clone();
    let task = tokio::spawn(async move {
        tokio::select! {
            _ = &mut shutdown_rx => {
                info!(session_id = %session_for_shutdown, "registry quic connection shutdown requested");
                control_for_shutdown.clear();
                pending_for_shutdown.0.lock().await.clear();
                emit_window(&window_for_shutdown, "registry:quic:ended", ());
            }
            _ = async move {
        let mut last_err: Option<anyhow::Error> = None;

        let Some(api_base) = api_base.clone() else {
            last_err = Some(anyhow!("missing api base"));
            if let Some(err) = last_err {
                warn!(error = %err, "registry quic connection failed");
                emit_window(&window_handle, "registry:quic:error", err.to_string());
            }
            return;
        };
        let connect_started_at = Instant::now();

        let socket = match UdpSocket::bind("0.0.0.0:0") {
            Ok(s) => s,
            Err(err) => {
                last_err = Some(anyhow!(err.to_string()));
                if let Some(err) = last_err {
                    warn!(error = %err, "registry quic connection failed");
                    emit_window(&window_handle, "registry:quic:error", err.to_string());
                }
                return;
            }
        };
        if let Err(err) = socket.set_nonblocking(true) {
            last_err = Some(anyhow!(err.to_string()));
            if let Some(err) = last_err {
                warn!(error = %err, "registry quic connection failed");
                emit_window(&window_handle, "registry:quic:error", err.to_string());
            }
            return;
        }

        let viewer_reflex = match tokio::task::spawn_blocking({
            let stun_socket = socket.try_clone().ok();
            move || -> Result<SocketAddr, anyhow::Error> {
                let stun_socket = stun_socket.ok_or_else(|| anyhow!("stun socket clone failed"))?;
                query_configured_stun_reflex(stun_socket)
            }
        })
        .await
        {
            Ok(Ok(addr)) => addr,
            Ok(Err(err)) => {
                last_err = Some(err);
                if let Some(err) = last_err {
                    warn!(error = %err, "registry quic connection failed");
                    emit_window(&window_handle, "registry:quic:error", err.to_string());
                }
                return;
            }
            Err(err) => {
                last_err = Some(anyhow!(err.to_string()));
                if let Some(err) = last_err {
                    warn!(error = %err, "registry quic connection failed");
                    emit_window(&window_handle, "registry:quic:error", err.to_string());
                }
                return;
            }
        };
        let url = format!(
            "{}/api/rmm/registry/session/{}/viewer-reflex?token={}",
            api_base,
            session_id,
            urlencoding::encode(&token)
        );
        let body = serde_json::json!({
            "ip": viewer_reflex.ip().to_string(),
            "port": viewer_reflex.port()
        });
        let reflex_resp = Client::new().post(url).json(&body).send().await;
        let reflex_resp = match reflex_resp {
            Ok(resp) => resp,
            Err(err) => {
                last_err = Some(anyhow!(err.to_string()));
                if let Some(err) = last_err {
                    warn!(error = %err, "registry quic connection failed");
                    emit_window(&window_handle, "registry:quic:error", err.to_string());
                }
                return;
            }
        };
        let reflex_status = reflex_resp.status().as_u16();
        if !(200..300).contains(&reflex_status) {
            let reflex_text = reflex_resp.text().await.unwrap_or_default();
            let body_prefix: String = reflex_text.chars().take(200).collect();
            last_err = Some(anyhow!("viewer-reflex non-2xx ({reflex_status}): {body_prefix}"));
            if let Some(err) = last_err {
                warn!(error = %err, "registry quic connection failed");
                emit_window(&window_handle, "registry:quic:error", err.to_string());
            }
            return;
        }

        let mut endpoint = match Endpoint::new(
            EndpointConfig::default(),
            None,
            socket,
            Arc::new(TokioRuntime),
        ) {
            Ok(ep) => ep,
            Err(err) => {
                last_err = Some(anyhow!(err.to_string()));
                if let Some(err) = last_err {
                    warn!(error = %err, "registry quic connection failed");
                    emit_window(&window_handle, "registry:quic:error", err.to_string());
                }
                return;
            }
        };
        let client_config = match build_client_config(&psk_cert_pem) {
            Ok(cfg) => cfg,
            Err(err) => {
                last_err = Some(err);
                if let Some(err) = last_err {
                    warn!(error = %err, "registry quic connection failed");
                    emit_window(&window_handle, "registry:quic:error", err.to_string());
                }
                return;
            }
        };
        endpoint.set_default_client_config(client_config);

        let quic_timeout = Duration::from_millis(quic_timeout_ms.unwrap_or(500));
        let connect_and_run = |connection: Connection| async {
            let (control_tx, control_rx) = mpsc::unbounded_channel();
            control_state.set_sender(
                RegistryControlTransport::Quic,
                session_id.clone(),
                control_tx,
            );
            emit_window(
                &window_handle,
                "registry:quic:hello",
                format!("quic connected ({session_id})"),
            );
            let result = run_registry_quic_stream(
                window_handle.clone(),
                session_id.clone(),
                connection,
                control_state.clone(),
                pending_state.clone(),
                control_rx,
            )
            .await;
            control_state.clear_if_transport(RegistryControlTransport::Quic);
            pending_state.0.lock().await.clear();
            emit_window(&window_handle, "registry:quic:ended", ());
            result
        };

        if let Some(lan_addr) = lan_addr {
            let mut lan_handle = tokio::spawn(run_quic_with_timeout(
                endpoint.clone(),
                session_id.clone(),
                lan_addr,
                quic_timeout,
            ));
            let mut reflex_handle = tokio::spawn(run_quic_with_timeout(
                endpoint.clone(),
                session_id.clone(),
                reflex_addr,
                quic_timeout,
            ));

            let mut lan_done = false;
            let mut reflex_done = false;
            loop {
                tokio::select! {
                    result = &mut lan_handle, if !lan_done => {
                        match result {
                            Ok(Ok(connection)) => {
                                reflex_handle.abort();
                                start_connection_telemetry(
                                    window_handle.clone(),
                                    telemetry_state.clone(),
                                    ControlState::default(),
                                    ConnectionStatePayload {
                                        session_kind: "remote_registry".to_string(),
                                        transport: "quic".to_string(),
                                        connection_type: "lan_direct".to_string(),
                                        encryption_label: "Pinned QUIC TLS".to_string(),
                                        encryption_details: Some(
                                            "Direct QUIC session authenticated with the per-session pinned certificate.".to_string()
                                        ),
                                        remote_addr: Some(connection.remote_address().to_string()),
                                        viewer_reflex: Some(ReflexAddress {
                                            ip: viewer_reflex.ip().to_string(),
                                            port: viewer_reflex.port(),
                                        }),
                                        agent_reflex: Some(agent_reflex.clone()),
                                        agent_local_addrs: agent_local_addrs.clone().unwrap_or_default(),
                                        connect_ms: Some(
                                            connect_started_at
                                                .elapsed()
                                                .as_millis()
                                                .min(u128::from(u64::MAX))
                                                as u64,
                                        ),
                                        relay_tcp_ms: None,
                                        relay_tls_ms: None,
                                        relay_handshake_ms: None,
                                        capture_type: None,
                                    }
                                );
                                if let Err(err) = connect_and_run(connection).await {
                                    last_err = Some(err);
                                }
                                break;
                            }
                            Ok(Err(err)) => {
                                last_err = Some(err);
                                lan_done = true;
                            }
                            Err(err) => {
                                last_err = Some(anyhow!(err.to_string()));
                                lan_done = true;
                            }
                        }
                    }
                    result = &mut reflex_handle, if !reflex_done => {
                        match result {
                            Ok(Ok(connection)) => {
                                lan_handle.abort();
                                start_connection_telemetry(
                                    window_handle.clone(),
                                    telemetry_state.clone(),
                                    ControlState::default(),
                                    ConnectionStatePayload {
                                        session_kind: "remote_registry".to_string(),
                                        transport: "quic".to_string(),
                                        connection_type: "hole_punch".to_string(),
                                        encryption_label: "Pinned QUIC TLS".to_string(),
                                        encryption_details: Some(
                                            "Peer-to-peer QUIC session authenticated with the per-session pinned certificate.".to_string()
                                        ),
                                        remote_addr: Some(connection.remote_address().to_string()),
                                        viewer_reflex: Some(ReflexAddress {
                                            ip: viewer_reflex.ip().to_string(),
                                            port: viewer_reflex.port(),
                                        }),
                                        agent_reflex: Some(agent_reflex.clone()),
                                        agent_local_addrs: agent_local_addrs.clone().unwrap_or_default(),
                                        connect_ms: Some(
                                            connect_started_at
                                                .elapsed()
                                                .as_millis()
                                                .min(u128::from(u64::MAX))
                                                as u64,
                                        ),
                                        relay_tcp_ms: None,
                                        relay_tls_ms: None,
                                        relay_handshake_ms: None,
                                        capture_type: None,
                                    }
                                );
                                if let Err(err) = connect_and_run(connection).await {
                                    last_err = Some(err);
                                }
                                break;
                            }
                            Ok(Err(err)) => {
                                last_err = Some(err);
                                reflex_done = true;
                            }
                            Err(err) => {
                                last_err = Some(anyhow!(err.to_string()));
                                reflex_done = true;
                            }
                        }
                    }
                    else => break,
                }
            }
        } else {
            match run_quic_with_timeout(
                endpoint.clone(),
                session_id.clone(),
                reflex_addr,
                quic_timeout,
            )
            .await
            {
                Ok(connection) => {
                    start_connection_telemetry(
                        window_handle.clone(),
                        telemetry_state.clone(),
                        ControlState::default(),
                        ConnectionStatePayload {
                            session_kind: "remote_registry".to_string(),
                            transport: "quic".to_string(),
                            connection_type: "hole_punch".to_string(),
                            encryption_label: "Pinned QUIC TLS".to_string(),
                            encryption_details: Some(
                                "Peer-to-peer QUIC session authenticated with the per-session pinned certificate.".to_string()
                            ),
                            remote_addr: Some(connection.remote_address().to_string()),
                            viewer_reflex: Some(ReflexAddress {
                                ip: viewer_reflex.ip().to_string(),
                                port: viewer_reflex.port(),
                            }),
                            agent_reflex: Some(agent_reflex.clone()),
                            agent_local_addrs: agent_local_addrs.clone().unwrap_or_default(),
                            connect_ms: Some(
                                connect_started_at
                                    .elapsed()
                                    .as_millis()
                                    .min(u128::from(u64::MAX))
                                    as u64,
                            ),
                            relay_tcp_ms: None,
                            relay_tls_ms: None,
                            relay_handshake_ms: None,
                            capture_type: None,
                        }
                    );
                    if let Err(err) = connect_and_run(connection).await {
                        last_err = Some(err);
                    }
                }
                Err(err) => {
                    last_err = Some(err);
                }
            }
        }

        if let Some(err) = last_err {
            warn!(error = %err, "registry quic connection failed");
            emit_window(&window_handle, "registry:quic:error", err.to_string());
        }
            } => {}
        }
    });

    quic_state
        .0
        .lock()
        .map_err(|e| e.to_string())?
        .replace(QuicConnectionTask {
            handle: task,
            shutdown: shutdown_tx,
        });

    Ok(())
}

fn format_relay_error(err: &anyhow::Error) -> String {
    let msg = err.to_string();
    if msg.contains("actively refused")
        || msg.contains("10061")
        || msg.contains("connection refused")
    {
        format!(
            "{}\n\nHint: Connection refused — ensure the relay is running, its configured port is reachable, and RMM_RELAY_URL names that relay endpoint.",
            msg
        )
    } else if msg.contains("timed out") || msg.contains("timeout") {
        format!(
            "{}\n\nHint: Connection timed out — check firewall/network and that the relay host is reachable.",
            msg
        )
    } else if msg.contains("certificate")
        || msg.contains("certificate verify")
        || msg.contains("invalid certificate")
        || msg.contains("CertNotValidForName")
        || msg.contains("relay tls connect")
    {
        format!(
            "{}\n\nHint: TLS handshake failed (often certificate verification). Ensure the relay certificate chain is trusted by the OS and includes a SAN that matches the relay hostname in RMM_RELAY_URL.",
            msg
        )
    } else {
        msg
    }
}

async fn notify_session_end(context: SessionCloseContext) {
    let url = format!(
        "{}/api/rmm/session/{}/end?token={}",
        context.api_base.trim_end_matches('/'),
        context.session_id,
        urlencoding::encode(&context.token)
    );
    if let Err(err) = Client::new().post(url).send().await {
        debug!(error = %err, "session end notification failed");
    }
}

#[derive(Default, Clone)]
pub(crate) struct ControlState {
    inner: Arc<Mutex<ControlSession>>,
}

#[derive(Default)]
struct ControlSession {
    sender: Option<mpsc::UnboundedSender<Vec<u8>>>,
    stream_size: Option<(u32, u32)>,
}

impl ControlState {
    fn set_sender(&self, sender: Option<mpsc::UnboundedSender<Vec<u8>>>) {
        if let Ok(mut guard) = self.inner.lock() {
            guard.sender = sender;
        }
    }

    fn set_stream_size(&self, size: Option<(u32, u32)>) {
        if let Ok(mut guard) = self.inner.lock() {
            guard.stream_size = size;
        }
    }

    fn sender(&self) -> Option<mpsc::UnboundedSender<Vec<u8>>> {
        self.inner
            .lock()
            .ok()
            .and_then(|guard| guard.sender.clone())
    }

    fn stream_size(&self) -> Option<(u32, u32)> {
        self.inner.lock().ok().and_then(|guard| guard.stream_size)
    }

    fn clear(&self) {
        if let Ok(mut guard) = self.inner.lock() {
            guard.sender = None;
            guard.stream_size = None;
        }
    }
}

// ---------------------------------------------------------------------------
// Remote Registry: separate control + transport state (v1)
// ---------------------------------------------------------------------------

#[derive(Default, Clone)]
struct RegistryControlState {
    inner: Arc<Mutex<RegistryControlSession>>,
}

#[derive(Default)]
struct RegistryControlSession {
    sender: Option<(
        RegistryControlTransport,
        String,
        mpsc::UnboundedSender<Vec<u8>>,
    )>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RegistryControlTransport {
    Quic,
    Relay,
}

impl RegistryControlState {
    fn set_sender(
        &self,
        transport: RegistryControlTransport,
        session_id: String,
        sender: mpsc::UnboundedSender<Vec<u8>>,
    ) {
        if let Ok(mut guard) = self.inner.lock() {
            guard.sender = Some((transport, session_id, sender));
        }
    }

    fn sender(&self) -> Option<(String, mpsc::UnboundedSender<Vec<u8>>)> {
        self.inner.lock().ok().and_then(|guard| {
            guard
                .sender
                .as_ref()
                .map(|(_, session_id, sender)| (session_id.clone(), sender.clone()))
        })
    }

    fn clear_if_transport(&self, transport: RegistryControlTransport) {
        if let Ok(mut guard) = self.inner.lock() {
            if let Some((current, _, _)) = guard.sender.as_ref() {
                if *current == transport {
                    guard.sender = None;
                }
            }
        }
    }

    fn clear(&self) {
        if let Ok(mut guard) = self.inner.lock() {
            guard.sender = None;
        }
    }
}

/// Shared state: optional handle of the running registry relay task.
#[derive(Clone)]
struct RegistryRelayConnectionState(pub Arc<Mutex<Option<JoinHandle<()>>>>);

impl Default for RegistryRelayConnectionState {
    fn default() -> Self {
        Self(Arc::new(Mutex::new(None)))
    }
}

#[derive(Clone)]
struct RegistryQuicConnectionState(pub Arc<Mutex<Option<QuicConnectionTask>>>);

impl Default for RegistryQuicConnectionState {
    fn default() -> Self {
        Self(Arc::new(Mutex::new(None)))
    }
}

#[derive(Clone, Default)]
struct RemoteRegistryPendingState(
    pub Arc<AsyncMutex<HashMap<String, oneshot::Sender<RegistryResponseEnvelope>>>>,
);

/// Shared state: optional handle of the running relay task so we can abort it on "End session".
#[derive(Clone)]
struct RelayConnectionState(pub Arc<Mutex<Option<JoinHandle<()>>>>);

impl Default for RelayConnectionState {
    fn default() -> Self {
        Self(Arc::new(Mutex::new(None)))
    }
}

struct QuicConnectionTask {
    handle: JoinHandle<()>,
    shutdown: oneshot::Sender<()>,
}

#[derive(Clone)]
struct QuicConnectionState(pub Arc<Mutex<Option<QuicConnectionTask>>>);

impl Default for QuicConnectionState {
    fn default() -> Self {
        Self(Arc::new(Mutex::new(None)))
    }
}

#[derive(Clone)]
struct SessionCloseContext {
    session_id: String,
    token: String,
    api_base: String,
}

#[derive(Clone)]
struct SessionCloseState(pub Arc<Mutex<Option<SessionCloseContext>>>);

impl Default for SessionCloseState {
    fn default() -> Self {
        Self(Arc::new(Mutex::new(None)))
    }
}

fn set_session_close_context(
    state: &SessionCloseState,
    session_id: String,
    token: String,
    api_base: Option<String>,
) {
    let Some(api_base) = api_base.map(|v| v.trim().to_string()) else {
        return;
    };
    if api_base.is_empty() || token.trim().is_empty() {
        return;
    }
    if let Ok(mut guard) = state.0.lock() {
        *guard = Some(SessionCloseContext {
            session_id,
            token,
            api_base,
        });
    }
}

#[tauri::command]
async fn connect_relay(
    window: Window,
    window_states: State<'_, AppWindowStates>,
    session_id: String,
    relay_url: String,
    e2e_key: String,
    token: String,
    api_base: Option<String>,
    selected_stream_protocol: Option<String>,
) -> Result<(), String> {
    let state = window_states.get_or_create(window.label());
    let relay_state = state.relay.clone();
    let control_state = state.control.clone();
    let telemetry_state = state.remote_connection_telemetry.clone();
    let registry_pending = state.registry_pending.clone();
    let viewport = state.viewport_handle();

    set_session_close_context(&state.session_close, session_id.clone(), token, api_base);

    if let Some(existing) = relay_state.0.lock().map_err(|e| e.to_string())?.take() {
        existing.abort();
    }

    let state_arc = relay_state.0.clone();
    let window_clone = window.clone();
    let stream_protocol =
        selected_stream_protocol.unwrap_or_else(|| REMOTE_DESKTOP_PROTOCOL_LEGACY_IVF.to_string());
    let handle = tokio::spawn(async move {
        let (control_tx, control_rx) = mpsc::unbounded_channel();
        control_state.set_sender(Some(control_tx));
        control_state.set_stream_size(None);
        let result = run_relay_connection(
            window_clone.clone(),
            session_id,
            relay_url,
            e2e_key,
            control_state.clone(),
            registry_pending.clone(),
            telemetry_state.clone(),
            viewport,
            control_rx,
            stream_protocol,
        )
        .await;
        telemetry_state.clear_transport("relay");
        if let Ok(mut guard) = state_arc.lock() {
            guard.take();
        }
        if let Err(err) = result {
            let msg = format_relay_error(&err);
            emit_window(&window_clone, "relay:error", msg);
        }
    });
    relay_state
        .0
        .lock()
        .map_err(|e| e.to_string())?
        .replace(handle);
    Ok(())
}

#[tauri::command]
async fn registry_connect_relay(
    window: Window,
    window_states: State<'_, AppWindowStates>,
    session_id: String,
    relay_url: String,
    e2e_key: String,
) -> Result<(), String> {
    let state = window_states.get_or_create(window.label());
    let relay_state = state.registry_relay.clone();
    let control_state = state.registry_control.clone();
    let pending = state.remote_registry_pending.clone();
    let telemetry_state = state.registry_connection_telemetry.clone();

    if let Some(existing) = relay_state.0.lock().map_err(|e| e.to_string())?.take() {
        existing.abort();
    }

    pending.0.lock().await.clear();
    control_state.clear();

    let state_arc = relay_state.0.clone();
    let window_clone = window.clone();
    let handle = tokio::spawn(async move {
        let (control_tx, control_rx) = mpsc::unbounded_channel();
        control_state.set_sender(
            RegistryControlTransport::Relay,
            session_id.clone(),
            control_tx,
        );
        let result = run_registry_relay_connection(
            window_clone.clone(),
            session_id,
            relay_url,
            e2e_key,
            telemetry_state.clone(),
            control_state.clone(),
            pending.clone(),
            control_rx,
        )
        .await;

        control_state.clear_if_transport(RegistryControlTransport::Relay);
        pending.0.lock().await.clear();

        if let Ok(mut guard) = state_arc.lock() {
            guard.take();
        }

        match result {
            Ok(()) => {
                emit_window(&window_clone, "registry:relay:ended", ());
            }
            Err(err) => {
                let msg = format_relay_error(&err);
                emit_window(&window_clone, "registry:relay:error", msg);
                emit_window(&window_clone, "registry:relay:ended", ());
            }
        }
    });

    relay_state
        .0
        .lock()
        .map_err(|e| e.to_string())?
        .replace(handle);
    Ok(())
}

#[tauri::command]
async fn disconnect_relay(
    window: Window,
    window_states: State<'_, AppWindowStates>,
) -> Result<(), String> {
    force_release_forwarded_win_key("main.rs:disconnect_relay");
    let state = window_states.get_or_create(window.label());
    if let Some(handle) = state.relay.0.lock().map_err(|e| e.to_string())?.take() {
        handle.abort();
    }
    state.remote_connection_telemetry.clear_transport("relay");
    state.registry_pending.0.lock().await.clear();
    emit_window(&window, "relay:ended", ());
    Ok(())
}

#[tauri::command]
async fn disconnect_quic(
    window: Window,
    window_states: State<'_, AppWindowStates>,
) -> Result<(), String> {
    force_release_forwarded_win_key("main.rs:disconnect_quic");
    let state = window_states.get_or_create(window.label());
    if let Some(task) = state.quic.0.lock().map_err(|e| e.to_string())?.take() {
        let _ = task.shutdown.send(());
        task.handle.abort();
    }
    state.remote_connection_telemetry.clear_transport("quic");
    state.control.clear();
    state.registry_pending.0.lock().await.clear();
    emit_window(&window, "quic:ended", ());
    Ok(())
}

#[tauri::command]
async fn registry_disconnect_relay(
    window: Window,
    window_states: State<'_, AppWindowStates>,
) -> Result<(), String> {
    let state = window_states.get_or_create(window.label());
    if let Some(handle) = state
        .registry_relay
        .0
        .lock()
        .map_err(|e| e.to_string())?
        .take()
    {
        handle.abort();
    }
    state
        .registry_control
        .clear_if_transport(RegistryControlTransport::Relay);
    state.registry_connection_telemetry.clear_transport("relay");
    state.remote_registry_pending.0.lock().await.clear();
    emit_window(&window, "registry:relay:ended", ());
    Ok(())
}

#[tauri::command]
async fn registry_disconnect_quic(
    window: Window,
    window_states: State<'_, AppWindowStates>,
) -> Result<(), String> {
    let state = window_states.get_or_create(window.label());
    if let Some(task) = state
        .registry_quic
        .0
        .lock()
        .map_err(|e| e.to_string())?
        .take()
    {
        let _ = task.shutdown.send(());
        task.handle.abort();
    }
    state
        .registry_control
        .clear_if_transport(RegistryControlTransport::Quic);
    state.registry_connection_telemetry.clear_transport("quic");
    state.remote_registry_pending.0.lock().await.clear();
    emit_window(&window, "registry:quic:ended", ());
    Ok(())
}

/// Clears control state so viewport input stops. Call on End session only.
#[tauri::command]
fn clear_control_state(
    window: Window,
    window_states: State<'_, AppWindowStates>,
) -> Result<(), String> {
    force_release_forwarded_win_key("main.rs:clear_control_state");
    let state = window_states.get_or_create(window.label());
    state.control.clear();
    Ok(())
}

pub(crate) async fn run_quic_with_timeout(
    endpoint: Endpoint,
    session_id: String,
    server_addr: SocketAddr,
    quic_timeout: Duration,
) -> Result<Connection, anyhow::Error> {
    match timeout(
        quic_timeout,
        run_quic_connect(endpoint, session_id, server_addr),
    )
    .await
    {
        Ok(Ok(connection)) => Ok(connection),
        Ok(Err(err)) => Err(err),
        Err(_) => Err(anyhow!(
            "connection timeout ({}ms)",
            quic_timeout.as_millis()
        )),
    }
}

async fn run_quic_connect(
    endpoint: Endpoint,
    session_id: String,
    server_addr: SocketAddr,
) -> Result<Connection, anyhow::Error> {
    info!(
        session_id = %session_id,
        target = %server_addr,
        "quic connect initiated"
    );

    let connection = endpoint
        .connect(server_addr, "rmm.local")
        .context("start quic connection")?
        .await
        .context("await quic connection")?;
    info!(session_id = %session_id, "quic connected");
    Ok(connection)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RdpSessionInfo {
    session_id: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    logical_session_id: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    native_session_id: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    win_station: Option<String>,
    user_name: String,
    state: String,
}

#[derive(Debug, Deserialize)]
struct RdpSessionsWireEnvelope {
    #[serde(rename = "type")]
    message_type: String,
    sessions: Vec<RdpSessionInfo>,
}

#[derive(Debug, Clone, Serialize)]
struct RdpSessionsEventPayload {
    sessions: Vec<RdpSessionInfo>,
}

#[derive(Clone, Default)]
struct RegistryPendingState(
    pub Arc<AsyncMutex<HashMap<String, oneshot::Sender<RegistryResponseEnvelope>>>>,
);

static REGISTRY_REQUEST_COUNTER: AtomicU64 = AtomicU64::new(1);

fn next_registry_request_id() -> String {
    let id = REGISTRY_REQUEST_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("reg-{id}")
}

fn registry_request_id(request: &RegistryRequest) -> &str {
    match request {
        RegistryRequest::ListKeys { request_id, .. } => request_id,
        RegistryRequest::ListValues { request_id, .. } => request_id,
        RegistryRequest::GetValue { request_id, .. } => request_id,
        RegistryRequest::SetValue { request_id, .. } => request_id,
        RegistryRequest::CreateKey { request_id, .. } => request_id,
        RegistryRequest::DeleteKey { request_id, .. } => request_id,
        RegistryRequest::DeleteValue { request_id, .. } => request_id,
        RegistryRequest::Cancel { request_id, .. } => request_id,
    }
}

fn registry_request_session_id(request: &RegistryRequest) -> &str {
    match request {
        RegistryRequest::ListKeys { session_id, .. } => session_id,
        RegistryRequest::ListValues { session_id, .. } => session_id,
        RegistryRequest::GetValue { session_id, .. } => session_id,
        RegistryRequest::SetValue { session_id, .. } => session_id,
        RegistryRequest::CreateKey { session_id, .. } => session_id,
        RegistryRequest::DeleteKey { session_id, .. } => session_id,
        RegistryRequest::DeleteValue { session_id, .. } => session_id,
        RegistryRequest::Cancel { session_id, .. } => session_id,
    }
}

fn extract_rmmd_meta(payload: &[u8]) -> Option<&[u8]> {
    if payload.len() < 8 || payload.get(0..4) != Some(b"RMMD") {
        return None;
    }
    let len = u32::from_le_bytes([payload[4], payload[5], payload[6], payload[7]]) as usize;
    if payload.len() < 8 + len {
        return None;
    }
    Some(&payload[8..8 + len])
}

fn capture_outputs_event_payload(
    parsed: &RemoteDesktopStreamMetaPayload,
) -> Option<CaptureOutputsEventPayload> {
    let active_index = parsed.active_index?;
    if parsed.capture_outputs.is_empty() {
        return None;
    }
    Some(CaptureOutputsEventPayload {
        outputs: parsed.capture_outputs.clone(),
        active_index,
        capture_type: parsed.capture_type.clone(),
    })
}

fn emit_rdp_sessions_if_present(app: &Window, payload: &[u8]) -> bool {
    // Avoid JSON parsing on video/binary payloads.
    if payload.first().copied() != Some(b'{') {
        return false;
    }
    let parsed = match serde_json::from_slice::<RdpSessionsWireEnvelope>(payload) {
        Ok(v) => v,
        Err(_) => return false,
    };
    if parsed.message_type != "rdp_sessions" {
        return false;
    }
    let session_count = parsed.sessions.len();
    emit_window(
        app,
        "rdp_sessions",
        RdpSessionsEventPayload {
            sessions: parsed.sessions,
        },
    );
    info!(session_count = session_count, "rdp_sessions event emitted");
    true
}

fn emit_connection_pong_if_present(
    app: &Window,
    telemetry: &ConnectionTelemetryState,
    payload: &[u8],
) -> bool {
    if payload.first().copied() != Some(b'{') {
        return false;
    }
    let parsed = match serde_json::from_slice::<ConnectionPongMetaPayload>(payload) {
        Ok(value) => value,
        Err(_) => return false,
    };
    if parsed.message_type != "connection_pong" {
        return false;
    }
    let now_ms = current_unix_ms();
    let rtt_ms = now_ms.saturating_sub(parsed.echoed_at_ms) as f64;
    telemetry.record_rtt(rtt_ms);
    emit_connection_stats(app, telemetry);
    true
}

async fn handle_remote_registry_response_if_present(
    control_state: &RegistryControlState,
    pending: &RemoteRegistryPendingState,
    payload: &[u8],
) -> bool {
    if payload.first().copied() != Some(b'{') {
        return false;
    }
    let parsed = match serde_json::from_slice::<RegistryResponseEnvelope>(payload) {
        Ok(v) => v,
        Err(_) => return false,
    };
    if parsed.message_type != REGISTRY_META_MESSAGE_TYPE {
        return false;
    }
    let Some((current_session_id, _)) = control_state.sender() else {
        return false;
    };
    if parsed.session_id != current_session_id {
        warn!(
            response_session_id = %parsed.session_id,
            current_session_id = %current_session_id,
            request_id = %parsed.request_id,
            "discarding stale registry response for inactive session"
        );
        return true;
    }
    let request_id = parsed.request_id.clone();
    let sender = {
        let mut guard = pending.0.lock().await;
        guard.remove(&request_id)
    };
    if let Some(tx) = sender {
        let _ = tx.send(parsed);
    }
    true
}

async fn handle_remote_desktop_meta_if_present(
    app: &Window,
    _pending: &RegistryPendingState,
    telemetry: &ConnectionTelemetryState,
    payload: &[u8],
) -> bool {
    if let Ok(parsed) = serde_json::from_slice::<RemoteDesktopStreamMetaPayload>(payload) {
        let mut handled = false;
        if let Some(capture_type) = parsed.capture_type.as_deref() {
            if telemetry.update_capture_type(capture_type).is_some() {
                emit_connection_stats(app, telemetry);
            }
            handled = true;
        }
        if let Some(outputs_payload) = capture_outputs_event_payload(&parsed) {
            let output_count = outputs_payload.outputs.len();
            let active_index = outputs_payload.active_index;
            emit_window(app, "capture_outputs", outputs_payload);
            info!(output_count, active_index, "capture_outputs event emitted");
            handled = true;
        }
        if handled {
            return true;
        }
    }
    if emit_rdp_sessions_if_present(app, payload) {
        return true;
    }
    if emit_connection_pong_if_present(app, telemetry, payload) {
        return true;
    }
    false
}

async fn send_registry_request_and_wait(
    control_state: &RegistryControlState,
    pending: &RemoteRegistryPendingState,
    request: RegistryRequest,
    timeout_ms: u64,
) -> Result<RegistryResponseEnvelope, String> {
    let (current_session_id, sender) = match control_state.sender() {
        Some(sender) => sender,
        None => {
            return Err("Remote registry session is not connected".to_string());
        }
    };
    let request_session_id = registry_request_session_id(&request);
    if request_session_id != current_session_id {
        return Err(format!(
            "Remote registry session changed while preparing request (expected {request_session_id}, active {current_session_id})"
        ));
    }
    let payload =
        serde_json::to_vec(&request).map_err(|e| format!("serialize registry request: {e}"))?;
    let request_id = registry_request_id(&request).to_string();
    let frame = build_control_frame(CONTROL_TYPE_REGISTRY_REQUEST, &payload)
        .map_err(|e| format!("build registry control frame: {e}"))?;

    let (tx, rx) = oneshot::channel();
    {
        let mut guard = pending.0.lock().await;
        guard.insert(request_id.clone(), tx);
    }

    if sender.send(frame).is_err() {
        pending.0.lock().await.remove(&request_id);
        return Err("Registry control channel closed".to_string());
    }

    match timeout(Duration::from_millis(timeout_ms), rx).await {
        Ok(Ok(response)) => Ok(response),
        Ok(Err(_)) => Err("Registry request cancelled".to_string()),
        Err(_) => {
            pending.0.lock().await.remove(&request_id);
            let cancel_request = RegistryRequest::Cancel {
                request_id: next_registry_request_id(),
                session_id: current_session_id,
                target_request_id: request_id.clone(),
            };
            if let Ok(cancel_payload) = serde_json::to_vec(&cancel_request) {
                if let Ok(cancel_frame) =
                    build_control_frame(CONTROL_TYPE_REGISTRY_REQUEST, &cancel_payload)
                {
                    let _ = sender.send(cancel_frame);
                }
            }
            Err(format!("Registry request timed out ({timeout_ms}ms)"))
        }
    }
}

fn format_registry_response_error(
    code: talos_protocol::RegistryErrorCode,
    message: String,
) -> String {
    format!("Remote registry error ({code:?}): {message}")
}

fn active_registry_session_id(control_state: &RegistryControlState) -> Result<String, String> {
    control_state
        .sender()
        .map(|(session_id, _)| session_id)
        .ok_or_else(|| "Remote registry session is not connected".to_string())
}

const REGISTRY_PAGE_LIMIT: u32 = 256;

#[tauri::command]
async fn registry_list_keys(
    window: Window,
    window_states: State<'_, AppWindowStates>,
    hive: RegistryHive,
    path: String,
    timeout_ms: Option<u64>,
) -> Result<Vec<String>, String> {
    let state = window_states.get_or_create(window.label());
    let control_state = state.registry_control;
    let registry_pending = state.remote_registry_pending;
    let session_id = active_registry_session_id(&control_state)?;
    let timeout_ms = timeout_ms.unwrap_or(2500);
    let mut offset = 0u32;
    let mut all_keys = Vec::new();

    loop {
        let request = RegistryRequest::ListKeys {
            request_id: next_registry_request_id(),
            session_id: session_id.clone(),
            hive,
            path: path.clone(),
            offset,
            limit: REGISTRY_PAGE_LIMIT,
        };
        let envelope =
            send_registry_request_and_wait(&control_state, &registry_pending, request, timeout_ms)
                .await?;
        match envelope.response {
            RegistryResponse::ListKeys {
                keys, next_offset, ..
            } => {
                all_keys.extend(keys);
                if let Some(next_offset) = next_offset {
                    offset = next_offset;
                    continue;
                }
                return Ok(all_keys);
            }
            RegistryResponse::Error { code, message } => {
                return Err(format_registry_response_error(code, message));
            }
            other => return Err(format!("Unexpected registry response: {other:?}")),
        }
    }
}

#[tauri::command]
async fn registry_list_values(
    window: Window,
    window_states: State<'_, AppWindowStates>,
    hive: RegistryHive,
    path: String,
    timeout_ms: Option<u64>,
) -> Result<Vec<RegistryValueEntry>, String> {
    let state = window_states.get_or_create(window.label());
    let control_state = state.registry_control;
    let registry_pending = state.remote_registry_pending;
    let session_id = active_registry_session_id(&control_state)?;
    let timeout_ms = timeout_ms.unwrap_or(2500);
    let mut offset = 0u32;
    let mut all_values = Vec::new();

    loop {
        let request = RegistryRequest::ListValues {
            request_id: next_registry_request_id(),
            session_id: session_id.clone(),
            hive,
            path: path.clone(),
            offset,
            limit: REGISTRY_PAGE_LIMIT,
        };
        let envelope =
            send_registry_request_and_wait(&control_state, &registry_pending, request, timeout_ms)
                .await?;
        match envelope.response {
            RegistryResponse::ListValues {
                values,
                next_offset,
                ..
            } => {
                all_values.extend(values);
                if let Some(next_offset) = next_offset {
                    offset = next_offset;
                    continue;
                }
                return Ok(all_values);
            }
            RegistryResponse::Error { code, message } => {
                return Err(format_registry_response_error(code, message));
            }
            other => return Err(format!("Unexpected registry response: {other:?}")),
        }
    }
}

#[tauri::command]
async fn registry_get_value(
    window: Window,
    window_states: State<'_, AppWindowStates>,
    hive: RegistryHive,
    path: String,
    name: String,
    timeout_ms: Option<u64>,
) -> Result<Option<RegistryValueEntry>, String> {
    let state = window_states.get_or_create(window.label());
    let control_state = state.registry_control;
    let registry_pending = state.remote_registry_pending;
    let session_id = active_registry_session_id(&control_state)?;
    let request = RegistryRequest::GetValue {
        request_id: next_registry_request_id(),
        session_id,
        hive,
        path,
        name,
    };
    let envelope = send_registry_request_and_wait(
        &control_state,
        &registry_pending,
        request,
        timeout_ms.unwrap_or(2500),
    )
    .await?;
    match envelope.response {
        RegistryResponse::GetValue { value } => Ok(value),
        RegistryResponse::Error { code, message } => {
            Err(format_registry_response_error(code, message))
        }
        other => Err(format!("Unexpected registry response: {other:?}")),
    }
}

#[tauri::command]
async fn registry_set_value(
    window: Window,
    window_states: State<'_, AppWindowStates>,
    hive: RegistryHive,
    path: String,
    name: String,
    data: RegistryValueData,
    timeout_ms: Option<u64>,
) -> Result<(), String> {
    let state = window_states.get_or_create(window.label());
    let control_state = state.registry_control;
    let registry_pending = state.remote_registry_pending;
    let session_id = active_registry_session_id(&control_state)?;
    let request = RegistryRequest::SetValue {
        request_id: next_registry_request_id(),
        session_id,
        hive,
        path,
        name,
        data,
    };
    let envelope = send_registry_request_and_wait(
        &control_state,
        &registry_pending,
        request,
        timeout_ms.unwrap_or(4000),
    )
    .await?;
    match envelope.response {
        RegistryResponse::Ok {} => Ok(()),
        RegistryResponse::Error { code, message } => {
            Err(format_registry_response_error(code, message))
        }
        other => Err(format!("Unexpected registry response: {other:?}")),
    }
}

#[tauri::command]
async fn registry_create_key(
    window: Window,
    window_states: State<'_, AppWindowStates>,
    hive: RegistryHive,
    path: String,
    timeout_ms: Option<u64>,
) -> Result<(), String> {
    let state = window_states.get_or_create(window.label());
    let control_state = state.registry_control;
    let registry_pending = state.remote_registry_pending;
    let session_id = active_registry_session_id(&control_state)?;
    let request = RegistryRequest::CreateKey {
        request_id: next_registry_request_id(),
        session_id,
        hive,
        path,
    };
    let envelope = send_registry_request_and_wait(
        &control_state,
        &registry_pending,
        request,
        timeout_ms.unwrap_or(4000),
    )
    .await?;
    match envelope.response {
        RegistryResponse::Ok {} => Ok(()),
        RegistryResponse::Error { code, message } => {
            Err(format_registry_response_error(code, message))
        }
        other => Err(format!("Unexpected registry response: {other:?}")),
    }
}

#[tauri::command]
async fn registry_delete_key(
    window: Window,
    window_states: State<'_, AppWindowStates>,
    hive: RegistryHive,
    path: String,
    recursive: bool,
    timeout_ms: Option<u64>,
) -> Result<(), String> {
    let state = window_states.get_or_create(window.label());
    let control_state = state.registry_control;
    let registry_pending = state.remote_registry_pending;
    let session_id = active_registry_session_id(&control_state)?;
    let request = RegistryRequest::DeleteKey {
        request_id: next_registry_request_id(),
        session_id,
        hive,
        path,
        recursive,
    };
    let envelope = send_registry_request_and_wait(
        &control_state,
        &registry_pending,
        request,
        timeout_ms.unwrap_or(4000),
    )
    .await?;
    match envelope.response {
        RegistryResponse::Ok {} => Ok(()),
        RegistryResponse::Error { code, message } => {
            Err(format_registry_response_error(code, message))
        }
        other => Err(format!("Unexpected registry response: {other:?}")),
    }
}

#[tauri::command]
async fn registry_delete_value(
    window: Window,
    window_states: State<'_, AppWindowStates>,
    hive: RegistryHive,
    path: String,
    name: String,
    timeout_ms: Option<u64>,
) -> Result<(), String> {
    let state = window_states.get_or_create(window.label());
    let control_state = state.registry_control;
    let registry_pending = state.remote_registry_pending;
    let session_id = active_registry_session_id(&control_state)?;
    let request = RegistryRequest::DeleteValue {
        request_id: next_registry_request_id(),
        session_id,
        hive,
        path,
        name,
    };
    let envelope = send_registry_request_and_wait(
        &control_state,
        &registry_pending,
        request,
        timeout_ms.unwrap_or(4000),
    )
    .await?;
    match envelope.response {
        RegistryResponse::Ok {} => Ok(()),
        RegistryResponse::Error { code, message } => {
            Err(format_registry_response_error(code, message))
        }
        other => Err(format!("Unexpected registry response: {other:?}")),
    }
}

async fn run_registry_quic_stream(
    _app: Window,
    _session_id: String,
    connection: Connection,
    control_state: RegistryControlState,
    registry_pending: RemoteRegistryPendingState,
    mut control_rx: mpsc::UnboundedReceiver<Vec<u8>>,
) -> Result<(), anyhow::Error> {
    let mut control_stream = connection
        .open_uni()
        .await
        .context("open registry control stream")?;
    tokio::spawn(async move {
        while let Some(frame) = control_rx.recv().await {
            if control_stream.write_all(&frame).await.is_err() {
                break;
            }
        }
        let _ = control_stream.finish();
    });

    let mut recv = connection
        .accept_uni()
        .await
        .context("accept registry quic stream")?;

    // Read first 4 bytes to distinguish metadata (RMMD) vs IVF header (DKIF).
    let mut prefix = [0u8; 4];
    recv.read_exact(&mut prefix)
        .await
        .context("read registry quic prefix")?;

    // Drain metadata blocks (if any) before IVF header.
    let mut header = [0u8; 32];
    if prefix == *b"RMMD" {
        loop {
            let mut len_buf = [0u8; 4];
            if recv.read_exact(&mut len_buf).await.is_err() {
                return Ok(());
            }
            let meta_len = u32::from_le_bytes(len_buf) as usize;
            let mut meta = vec![0u8; meta_len];
            if recv.read_exact(&mut meta).await.is_err() {
                return Ok(());
            }
            let _ = handle_remote_registry_response_if_present(
                &control_state,
                &registry_pending,
                &meta,
            )
            .await;
            if recv.read_exact(&mut prefix).await.is_err() {
                return Ok(());
            }
            if prefix == *b"RMMD" {
                continue;
            }
            header[0..4].copy_from_slice(&prefix);
            if recv.read_exact(&mut header[4..32]).await.is_err() {
                return Ok(());
            }
            break;
        }
    } else {
        header[0..4].copy_from_slice(&prefix);
        if recv.read_exact(&mut header[4..32]).await.is_err() {
            return Ok(());
        }
    }

    if header[0..4] != *b"DKIF" {
        // Not an IVF stream; nothing else to parse reliably.
        return Ok(());
    }

    // Drain frames to avoid backpressure; intercept RMMD metadata.
    let mut scratch: Vec<u8> = Vec::new();
    loop {
        let mut len_bytes = [0u8; 4];
        if recv.read_exact(&mut len_bytes).await.is_err() {
            break;
        }
        if len_bytes == *b"RMMD" {
            let mut meta_len_buf = [0u8; 4];
            if recv.read_exact(&mut meta_len_buf).await.is_err() {
                break;
            }
            let meta_len = u32::from_le_bytes(meta_len_buf) as usize;
            let mut meta = vec![0u8; meta_len];
            if recv.read_exact(&mut meta).await.is_err() {
                break;
            }
            let _ = handle_remote_registry_response_if_present(
                &control_state,
                &registry_pending,
                &meta,
            )
            .await;
            continue;
        }

        let payload_len = u32::from_le_bytes(len_bytes) as usize;
        let mut pts_bytes = [0u8; 8];
        if recv.read_exact(&mut pts_bytes).await.is_err() {
            break;
        }
        if scratch.len() < payload_len {
            scratch.resize(payload_len, 0u8);
        }
        if payload_len > 0 && recv.read_exact(&mut scratch[..payload_len]).await.is_err() {
            break;
        }
    }

    Ok(())
}

#[cfg(windows)]
async fn run_quic_modern_display_delta(
    app: &Window,
    session_id: &str,
    recv: &mut quinn::RecvStream,
    registry_pending: &RegistryPendingState,
    telemetry: &ConnectionTelemetryState,
    viewport: &ViewportArc,
    control_state: &ControlState,
) -> Result<(), anyhow::Error> {
    debug!(session_id = %session_id, "quic modern display-delta stream started");
    emit_window(
        app,
        "quic:hello",
        "Stream connected; waiting for first frame".to_string(),
    );
    let mut compositor = display_delta::ModernDisplayCompositor::new();
    compositor.set_experimental_summary_context(session_id, "quic");
    let mut record_count: u64 = 0;
    let mut first_frame_presented = false;
    loop {
        let mut first = [0u8; 1];
        if recv.read_exact(&mut first).await.is_err() {
            break;
        }
        if first[0] == b'R' {
            let mut rest = [0u8; 3];
            if recv.read_exact(&mut rest).await.is_err() {
                break;
            }
            if rest == *b"MMD" {
                let mut len_buf = [0u8; 4];
                if recv.read_exact(&mut len_buf).await.is_err() {
                    break;
                }
                let meta_len = u32::from_le_bytes(len_buf) as usize;
                let mut meta = vec![0u8; meta_len];
                if recv.read_exact(&mut meta).await.is_err() {
                    break;
                }
                let _ =
                    handle_remote_desktop_meta_if_present(app, registry_pending, telemetry, &meta)
                        .await;
                continue;
            }
            let mut len_buf = [rest[0], rest[1], rest[2], 0];
            if recv.read_exact(&mut len_buf[3..4]).await.is_err() {
                break;
            }
            if let Err(err) = read_and_apply_display_record(
                app,
                session_id,
                recv,
                &mut compositor,
                viewport,
                Some(control_state),
                first[0],
                len_buf,
                &mut record_count,
                &mut first_frame_presented,
            )
            .await
            {
                compositor.log_experimental_summary(session_id, "quic", "error");
                return Err(err);
            }
            continue;
        }

        let mut len_buf = [0u8; 4];
        if recv.read_exact(&mut len_buf).await.is_err() {
            break;
        }
        if let Err(err) = read_and_apply_display_record(
            app,
            session_id,
            recv,
            &mut compositor,
            viewport,
            Some(control_state),
            first[0],
            len_buf,
            &mut record_count,
            &mut first_frame_presented,
        )
        .await
        {
            compositor.log_experimental_summary(session_id, "quic", "error");
            return Err(err);
        }
    }
    compositor.log_experimental_summary(session_id, "quic", "normal");
    info!(session_id = %session_id, record_count, "quic modern display-delta stream received");
    Ok(())
}

#[cfg(not(windows))]
async fn run_quic_modern_display_delta(
    app: &Window,
    session_id: &str,
    recv: &mut quinn::RecvStream,
    registry_pending: &RegistryPendingState,
    telemetry: &ConnectionTelemetryState,
    viewport: &ViewportArc,
    control_state: &ControlState,
) -> Result<(), anyhow::Error> {
    debug!(session_id = %session_id, "quic macOS display-delta stream started");
    emit_window(
        app,
        "quic:hello",
        "Stream connected; waiting for first frame".to_string(),
    );
    let mut compositor = ModernDisplayCompositor::new();
    let mut record_count = 0u64;
    let mut first_frame_presented = false;
    loop {
        let mut first = [0u8; 1];
        if recv.read_exact(&mut first).await.is_err() {
            break;
        }
        if first[0] == b'R' {
            let mut rest = [0u8; 3];
            if recv.read_exact(&mut rest).await.is_err() {
                break;
            }
            if rest == *b"MMD" {
                let mut len_buf = [0u8; 4];
                if recv.read_exact(&mut len_buf).await.is_err() {
                    break;
                }
                let meta_len = u32::from_le_bytes(len_buf) as usize;
                let mut meta = vec![0u8; meta_len];
                if recv.read_exact(&mut meta).await.is_err() {
                    break;
                }
                let _ =
                    handle_remote_desktop_meta_if_present(app, registry_pending, telemetry, &meta)
                        .await;
                continue;
            }
            let mut len_buf = [rest[0], rest[1], rest[2], 0];
            if recv.read_exact(&mut len_buf[3..4]).await.is_err() {
                break;
            }
            read_and_present_display_record_nonwindows(
                app,
                viewport,
                recv,
                &mut compositor,
                control_state,
                first[0],
                len_buf,
                &mut record_count,
                &mut first_frame_presented,
            )
            .await?;
            continue;
        }

        let mut len_buf = [0u8; 4];
        if recv.read_exact(&mut len_buf).await.is_err() {
            break;
        }
        read_and_present_display_record_nonwindows(
            app,
            viewport,
            recv,
            &mut compositor,
            control_state,
            first[0],
            len_buf,
            &mut record_count,
            &mut first_frame_presented,
        )
        .await?;
    }
    info!(session_id = %session_id, record_count, "quic macOS display-delta stream received");
    Ok(())
}

#[cfg(not(windows))]
async fn read_and_present_display_record_nonwindows(
    app: &Window,
    viewport: &ViewportArc,
    recv: &mut quinn::RecvStream,
    compositor: &mut ModernDisplayCompositor,
    control_state: &ControlState,
    message_type: u8,
    len_buf: [u8; 4],
    record_count: &mut u64,
    first_frame_presented: &mut bool,
) -> Result<(), anyhow::Error> {
    let payload_len = u32::from_le_bytes(len_buf) as usize;
    if payload_len > 128 * 1024 * 1024 {
        return Err(anyhow!("display record payload too large"));
    }
    let mut record = Vec::with_capacity(5 + payload_len);
    record.push(message_type);
    record.extend_from_slice(&len_buf);
    record.resize(5 + payload_len, 0);
    if payload_len > 0 {
        recv.read_exact(&mut record[5..])
            .await
            .context("read display record payload")?;
    }
    let frame = compositor.handle_record(&record).map_err(|err| {
        if let Ok(path) = std::env::var("TALOS_VIEWER_DUMP_ERROR") {
            if !path.trim().is_empty() {
                let _ = fs::write(
                    path,
                    format!("message_type={message_type} payload_len={payload_len} error={err}\n"),
                );
            }
        }
        anyhow!(err)
    })?;
    if let Some(frame) = frame {
        present_or_emit_remote_desktop_frame(app, viewport, &frame).map_err(|err| anyhow!(err))?;
        if !*first_frame_presented {
            *first_frame_presented = true;
            emit_window(app, "quic:hello", "First frame rendered".to_string());
        }
    }
    if let Some(dimensions) = compositor.dimensions() {
        control_state.set_stream_size(Some(dimensions));
    }
    *record_count = record_count.saturating_add(1);
    Ok(())
}

#[cfg(windows)]
async fn flush_deferred_experimental_display_if_needed(
    session_id: &str,
    transport: &'static str,
    compositor: &mut display_delta::ModernDisplayCompositor,
    viewport: &ViewportArc,
) -> Result<(), anyhow::Error> {
    if !compositor.has_deferred_experimental_frames() {
        return Ok(());
    }

    const MAX_ATTEMPTS: u32 = 80;
    const RETRY_DELAY: Duration = Duration::from_millis(25);
    for attempt in 0..MAX_ATTEMPTS {
        compositor
            .flush_deferred_experimental_frames(viewport)
            .map_err(|err| anyhow!(err))?;
        if !compositor.has_deferred_experimental_frames() {
            debug!(
                session_id = %session_id,
                transport,
                attempts = attempt + 1,
                "viewer experimental deferred display flushed"
            );
            return Ok(());
        }
        sleep(RETRY_DELAY).await;
    }

    debug!(
        session_id = %session_id,
        transport,
        pending_frames = compositor.deferred_experimental_frame_count(),
        "viewer experimental deferred display still waiting for viewport"
    );
    Ok(())
}

#[cfg(windows)]
async fn read_and_apply_display_record(
    app: &Window,
    session_id: &str,
    recv: &mut quinn::RecvStream,
    compositor: &mut display_delta::ModernDisplayCompositor,
    viewport: &ViewportArc,
    control_state: Option<&ControlState>,
    message_type: u8,
    len_buf: [u8; 4],
    record_count: &mut u64,
    first_frame_presented: &mut bool,
) -> Result<(), anyhow::Error> {
    let total_started = Instant::now();
    let payload_len = u32::from_le_bytes(len_buf) as usize;
    if payload_len > 128 * 1024 * 1024 {
        return Err(anyhow!("display record payload too large"));
    }
    debug!(
        session_id = %session_id,
        record_index = *record_count,
        message_type,
        payload_len,
        "viewer modern display record received"
    );
    let alloc_started = Instant::now();
    let mut record = Vec::with_capacity(5 + payload_len);
    record.push(message_type);
    record.extend_from_slice(&len_buf);
    record.resize(5 + payload_len, 0);
    let alloc_elapsed = alloc_started.elapsed();
    let read_started = Instant::now();
    if payload_len > 0 && recv.read_exact(&mut record[5..]).await.is_err() {
        return Err(anyhow!("read display record payload"));
    }
    let read_elapsed = read_started.elapsed();
    let handle_started = Instant::now();
    compositor
        .handle_record(session_id, viewport, &record)
        .map_err(|err| anyhow!(err))?;
    let handle_elapsed = handle_started.elapsed();
    if let (Some(control_state), Some(dimensions)) = (control_state, compositor.dimensions()) {
        control_state.set_stream_size(Some(dimensions));
    }
    let flush_started = Instant::now();
    flush_deferred_experimental_display_if_needed(session_id, "quic", compositor, viewport).await?;
    let flush_elapsed = flush_started.elapsed();
    if !*first_frame_presented && message_type == talos_protocol::DISPLAY_RECORD_FRAME_END {
        *first_frame_presented = true;
        emit_window(app, "quic:hello", "First frame rendered".to_string());
    }
    if payload_len >= 256 * 1024
        || message_type == talos_protocol::DISPLAY_RECORD_ATLAS_H264
        || message_type == talos_protocol::DISPLAY_RECORD_EXPERIMENTAL_ATLAS_COMMANDS
        || message_type == talos_protocol::DISPLAY_RECORD_EXPERIMENTAL_ATLAS_COMMANDS_CHUNK
    {
        display_delta::append_experimental_log(&format!(
            concat!(
                "viewer experimental quic record pipeline session_id={} record_index={} ",
                "message_type={} payload_len={} alloc_ms={:.3} read_ms={:.3} ",
                "handle_ms={:.3} flush_ms={:.3} total_ms={:.3}"
            ),
            session_id,
            *record_count,
            message_type,
            payload_len,
            alloc_elapsed.as_secs_f64() * 1000.0,
            read_elapsed.as_secs_f64() * 1000.0,
            handle_elapsed.as_secs_f64() * 1000.0,
            flush_elapsed.as_secs_f64() * 1000.0,
            total_started.elapsed().as_secs_f64() * 1000.0,
        ));
    }
    *record_count = record_count.saturating_add(1);
    Ok(())
}

async fn run_quic_stream(
    app: Window,
    session_id: String,
    connection: Connection,
    control_state: ControlState,
    registry_pending: RegistryPendingState,
    telemetry: ConnectionTelemetryState,
    viewport: ViewportArc,
    mut control_rx: mpsc::UnboundedReceiver<Vec<u8>>,
    stream_protocol: String,
) -> Result<(), anyhow::Error> {
    let (stats_shutdown_tx, mut stats_shutdown_rx) = oneshot::channel();
    let stats_window = app.clone();
    let stats_telemetry = telemetry.clone();
    let stats_connection = connection.clone();
    tokio::spawn(async move {
        let mut ticker = interval(Duration::from_secs(1));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                _ = &mut stats_shutdown_rx => break,
                _ = ticker.tick() => {
                    let rtt = stats_connection.rtt();
                    let rtt_ms = rtt.as_secs_f64() * 1000.0;
                    if rtt_ms.is_finite() && rtt_ms >= 0.0 {
                        stats_telemetry.record_rtt(rtt_ms);
                        emit_connection_stats(&stats_window, &stats_telemetry);
                    }
                }
            }
        }
    });
    let mut control_stream = connection.open_uni().await.context("open control stream")?;
    tokio::spawn(async move {
        while let Some(frame) = control_rx.recv().await {
            if control_stream.write_all(&frame).await.is_err() {
                break;
            }
        }
        let _ = control_stream.finish();
    });

    let mut recv = connection
        .accept_uni()
        .await
        .context("accept quic stream")?;
    let stream_protocol = stream_protocol.trim();
    if stream_protocol == REMOTE_DESKTOP_PROTOCOL_MODERN_DISPLAY_DELTA
        || stream_protocol == REMOTE_DESKTOP_PROTOCOL_EXPERIMENTAL_DISPLAY_DELTA
    {
        let result = run_quic_modern_display_delta(
            &app,
            &session_id,
            &mut recv,
            &registry_pending,
            &telemetry,
            &viewport,
            &control_state,
        )
        .await;
        let _ = stats_shutdown_tx.send(());
        return result;
    }
    // Read first 4 bytes to distinguish metadata (RMMD) vs IVF header (DKIF).
    let mut prefix = [0u8; 4];
    if let Err(_e) = recv.read_exact(&mut prefix).await {
        emit_window(&app, "quic:hello", "Hello from agent\n".to_string());
        return Ok(());
    }
    let mut header = [0u8; 32];
    if prefix == *b"RMMD" {
        loop {
            let mut len_buf = [0u8; 4];
            if recv.read_exact(&mut len_buf).await.is_err() {
                emit_window(&app, "quic:hello", "Hello from agent\n".to_string());
                return Ok(());
            }
            let meta_len = u32::from_le_bytes(len_buf) as usize;
            let mut meta = vec![0u8; meta_len];
            if recv.read_exact(&mut meta).await.is_err() {
                emit_window(&app, "quic:hello", "Hello from agent\n".to_string());
                return Ok(());
            }
            let _ =
                handle_remote_desktop_meta_if_present(&app, &registry_pending, &telemetry, &meta)
                    .await;
            if recv.read_exact(&mut prefix).await.is_err() {
                emit_window(&app, "quic:hello", "Hello from agent\n".to_string());
                return Ok(());
            }
            if prefix == *b"RMMD" {
                continue;
            }
            header[0..4].copy_from_slice(&prefix);
            if recv.read_exact(&mut header[4..32]).await.is_err() {
                emit_window(&app, "quic:hello", "Hello from agent\n".to_string());
                return Ok(());
            }
            break;
        }
    } else {
        header[0..4].copy_from_slice(&prefix);
        if recv.read_exact(&mut header[4..32]).await.is_err() {
            emit_window(&app, "quic:hello", "Hello from agent\n".to_string());
            return Ok(());
        }
    }
    if header[0..4] != *b"DKIF" {
        emit_window(&app, "quic:hello", "Hello from agent\n".to_string());
        return Ok(());
    }
    let (mut stream_width, mut stream_height, stream_fps) =
        parse_ivf_header(&header).ok_or_else(|| anyhow!("invalid IVF header"))?;
    control_state.set_stream_size(Some((stream_width, stream_height)));
    let mut decoder = Vp8Decoder::new().map_err(|err| anyhow!(err))?;
    debug!(
        session_id = %session_id,
        stream_width,
        stream_height,
        stream_fps,
        "viewer quic IVF stream header parsed; VP8 decoder initialized"
    );
    emit_window(
        &app,
        "quic:hello",
        "Stream connected; waiting for first frame".to_string(),
    );

    let mut frame_count: u64 = 0;
    let mut first_frame_presented = false;
    loop {
        let mut len_bytes = [0u8; 4];
        if let Err(_e) = recv.read_exact(&mut len_bytes).await {
            break;
        }
        if len_bytes == *b"RMMD" {
            let mut meta_len_buf = [0u8; 4];
            if recv.read_exact(&mut meta_len_buf).await.is_err() {
                break;
            }
            let meta_len = u32::from_le_bytes(meta_len_buf) as usize;
            let mut meta = vec![0u8; meta_len];
            if recv.read_exact(&mut meta).await.is_err() {
                break;
            }
            let _ =
                handle_remote_desktop_meta_if_present(&app, &registry_pending, &telemetry, &meta)
                    .await;
            continue;
        }
        let payload_len = match parse_legacy_vp8_payload_prefix(len_bytes) {
            LegacyVp8PayloadPrefix::Payload(payload_len) => payload_len,
            LegacyVp8PayloadPrefix::MidstreamIvfHeader => {
                warn!(
                    session_id = %session_id,
                    frame_index = frame_count,
                    "viewer quic IVF stream received unexpected midstream DKIF header"
                );
                break;
            }
            LegacyVp8PayloadPrefix::TooLarge(payload_len) => {
                warn!(
                    session_id = %session_id,
                    frame_index = frame_count,
                    payload_len,
                    max_payload_len = MAX_LEGACY_VP8_PAYLOAD_LEN,
                    "viewer quic IVF stream rejected oversized VP8 payload"
                );
                break;
            }
        };
        let mut pts_bytes = [0u8; 8];
        if let Err(_e) = recv.read_exact(&mut pts_bytes).await {
            break;
        }
        let mut payload = vec![0u8; payload_len];
        if let Err(_e) = recv.read_exact(&mut payload).await {
            break;
        }
        #[cfg(windows)]
        {
            if viewport_recently_moved(&viewport, 50).is_some() {
                debug!(
                    session_id = %session_id,
                    frame_index = frame_count,
                    payload_len,
                    "viewer quic VP8 frame skipped after viewport move"
                );
                frame_count += 1;
                continue;
            }
            debug!(
                session_id = %session_id,
                frame_index = frame_count,
                payload_len,
                "viewer quic VP8 decode input"
            );
            match decoder.decode(&payload) {
                Ok(Some(frame)) => {
                    let mut frame = frame;
                    frame.fps = stream_fps;
                    let frame_width = frame.width;
                    let frame_height = frame.height;
                    debug!(
                        session_id = %session_id,
                        frame_index = frame_count,
                        frame_width,
                        frame_height,
                        "viewer quic VP8 decode produced frame"
                    );
                    let session_id_for_present = session_id.clone();
                    let frame_index_for_present = frame_count;
                    let viewport = viewport.clone();
                    let _ = app.run_on_main_thread(move || {
                        let Ok(mut guard) = viewport.lock() else {
                            warn!(
                                session_id = %session_id_for_present,
                                frame_index = frame_index_for_present,
                                "viewer quic present skipped; viewport lock poisoned"
                            );
                            return;
                        };
                        debug!(
                            session_id = %session_id_for_present,
                            frame_index = frame_index_for_present,
                            has_last_rect = guard.last_rect.is_some(),
                            has_surface = guard.surface.is_some(),
                            has_gpu_viewport = guard.gpu_viewport.is_some(),
                            "viewer quic present dispatch on main thread"
                        );
                        let present_result = present_decoded_frame(&mut guard, &frame);
                        if let Err(err) = present_result {
                            warn!(
                                session_id = %session_id_for_present,
                                frame_index = frame_index_for_present,
                                error = %err,
                                "viewer quic viewport present failed"
                            );
                        } else {
                            debug!(
                                session_id = %session_id_for_present,
                                frame_index = frame_index_for_present,
                                "viewer quic viewport present returned"
                            );
                        }
                    });
                    if !first_frame_presented {
                        first_frame_presented = true;
                        emit_window(&app, "quic:hello", "First frame rendered".to_string());
                    }
                    if frame_width != stream_width || frame_height != stream_height {
                        stream_width = frame_width;
                        stream_height = frame_height;
                        control_state.set_stream_size(Some((stream_width, stream_height)));
                    }
                }
                Ok(None) => {
                    debug!(
                        session_id = %session_id,
                        frame_index = frame_count,
                        payload_len,
                        "viewer quic VP8 decode needs more input"
                    );
                }
                Err(err) => {
                    warn!(
                        session_id = %session_id,
                        frame_index = frame_count,
                        payload_len,
                        error = %err,
                        "viewer quic VP8 decode failed"
                    );
                }
            }
        }
        #[cfg(not(windows))]
        {
            debug!(
                session_id = %session_id,
                frame_index = frame_count,
                payload_len,
                "viewer quic VP8 decode input"
            );
            match decoder.decode(&payload) {
                Ok(Some(mut frame)) => {
                    frame.fps = stream_fps;
                    let frame_width = frame.width;
                    let frame_height = frame.height;
                    if let Err(err) = present_or_emit_remote_desktop_frame(&app, &viewport, &frame)
                    {
                        warn!(
                            session_id = %session_id,
                            frame_index = frame_count,
                            error = %err,
                            "viewer quic frame present failed"
                        );
                    } else if !first_frame_presented {
                        first_frame_presented = true;
                        emit_window(&app, "quic:hello", "First frame rendered".to_string());
                    }
                    if frame_width != stream_width || frame_height != stream_height {
                        stream_width = frame_width;
                        stream_height = frame_height;
                        control_state.set_stream_size(Some((stream_width, stream_height)));
                    }
                }
                Ok(None) => {}
                Err(err) => {
                    warn!(
                        session_id = %session_id,
                        frame_index = frame_count,
                        payload_len,
                        error = %err,
                        "viewer quic VP8 decode failed"
                    );
                }
            }
        }
        frame_count += 1;
    }

    info!(session_id = %session_id, frame_count, "quic IVF stream received");
    let _ = stats_shutdown_tx.send(());
    Ok(())
}

pub(crate) fn build_tls_config() -> Result<ClientConfig, anyhow::Error> {
    build_relay_client_tls_config(None, None)
}

pub(crate) fn relay_connect_timeout() -> Duration {
    Duration::from_secs(
        std::env::var("RMM_RELAY_CONNECT_TIMEOUT_SECS")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(10),
    )
}

#[cfg(windows)]
async fn run_relay_modern_display_delta<R>(
    app: &Window,
    session_id: &str,
    reader: &mut R,
    cipher: &ChaCha20Poly1305,
    registry_pending: &RegistryPendingState,
    telemetry: &ConnectionTelemetryState,
    viewport: &ViewportArc,
    control_state: &ControlState,
) -> Result<(), anyhow::Error>
where
    R: AsyncReadExt + Unpin,
{
    debug!(session_id = %session_id, "relay modern display-delta stream started");
    let mut compositor = display_delta::ModernDisplayCompositor::new();
    compositor.set_experimental_summary_context(session_id, "relay");
    let mut record_count: u64 = 0;
    let mut first_frame_presented = false;
    loop {
        let chunk = match read_e2e_frame_from(reader, cipher).await {
            Ok(chunk) => chunk,
            Err(e) => {
                let s = e.to_string();
                if s.contains("relay connection closed")
                    || s.contains("UnexpectedEof")
                    || s.contains("ConnectionReset")
                    || s.contains("BrokenPipe")
                    || s.contains("ConnectionAborted")
                {
                    break;
                }
                compositor.log_experimental_summary(session_id, "relay", "error");
                return Err(e);
            }
        };
        if let Some(meta) = extract_rmmd_meta(&chunk) {
            let _ =
                handle_remote_desktop_meta_if_present(app, registry_pending, telemetry, meta).await;
            continue;
        }
        if handle_remote_desktop_meta_if_present(app, registry_pending, telemetry, &chunk).await {
            continue;
        }
        debug!(
            session_id = %session_id,
            record_index = record_count,
            chunk_len = chunk.len(),
            "viewer relay modern display record received"
        );
        let total_started = Instant::now();
        let message_type = chunk.first().copied().unwrap_or_default();
        let payload_len = if chunk.len() >= 5 {
            u32::from_le_bytes([chunk[1], chunk[2], chunk[3], chunk[4]]) as usize
        } else {
            0
        };
        let handle_started = Instant::now();
        if let Err(err) = compositor
            .handle_record(session_id, viewport, &chunk)
            .map_err(|err| anyhow!(err))
        {
            compositor.log_experimental_summary(session_id, "relay", "error");
            return Err(err);
        }
        let handle_elapsed = handle_started.elapsed();
        if let Some(dimensions) = compositor.dimensions() {
            control_state.set_stream_size(Some(dimensions));
        }
        let flush_started = Instant::now();
        if let Err(err) = flush_deferred_experimental_display_if_needed(
            session_id,
            "relay",
            &mut compositor,
            viewport,
        )
        .await
        {
            compositor.log_experimental_summary(session_id, "relay", "error");
            return Err(err);
        }
        let flush_elapsed = flush_started.elapsed();
        if !first_frame_presented && message_type == talos_protocol::DISPLAY_RECORD_FRAME_END {
            first_frame_presented = true;
            emit_window(app, "relay:hello", "First frame rendered".to_string());
        }
        if chunk.len() >= 256 * 1024
            || message_type == talos_protocol::DISPLAY_RECORD_ATLAS_H264
            || message_type == talos_protocol::DISPLAY_RECORD_EXPERIMENTAL_ATLAS_COMMANDS
            || message_type == talos_protocol::DISPLAY_RECORD_EXPERIMENTAL_ATLAS_COMMANDS_CHUNK
        {
            display_delta::append_experimental_log(&format!(
                concat!(
                    "viewer experimental relay record pipeline session_id={} record_index={} ",
                    "message_type={} payload_len={} handle_ms={:.3} flush_ms={:.3} total_ms={:.3}"
                ),
                session_id,
                record_count,
                message_type,
                payload_len,
                handle_elapsed.as_secs_f64() * 1000.0,
                flush_elapsed.as_secs_f64() * 1000.0,
                total_started.elapsed().as_secs_f64() * 1000.0,
            ));
        }
        record_count = record_count.saturating_add(1);
    }
    compositor.log_experimental_summary(session_id, "relay", "normal");
    info!(session_id = %session_id, record_count, "relay modern display-delta stream received");
    Ok(())
}

#[cfg(not(windows))]
async fn run_relay_modern_display_delta<R>(
    app: &Window,
    session_id: &str,
    reader: &mut R,
    cipher: &ChaCha20Poly1305,
    registry_pending: &RegistryPendingState,
    telemetry: &ConnectionTelemetryState,
    viewport: &ViewportArc,
    control_state: &ControlState,
) -> Result<(), anyhow::Error>
where
    R: AsyncReadExt + Unpin,
{
    debug!(session_id = %session_id, "relay macOS display-delta stream started");
    emit_window(
        app,
        "relay:hello",
        "Stream connected; waiting for first frame".to_string(),
    );
    let mut compositor = ModernDisplayCompositor::new();
    let mut record_count = 0u64;
    let mut first_frame_presented = false;
    loop {
        let chunk = match read_e2e_frame_from(reader, cipher).await {
            Ok(chunk) => chunk,
            Err(e) => {
                let s = e.to_string();
                if s.contains("relay connection closed")
                    || s.contains("UnexpectedEof")
                    || s.contains("ConnectionReset")
                    || s.contains("BrokenPipe")
                    || s.contains("ConnectionAborted")
                {
                    break;
                }
                return Err(e);
            }
        };
        if let Some(meta) = extract_rmmd_meta(&chunk) {
            let _ =
                handle_remote_desktop_meta_if_present(app, registry_pending, telemetry, meta).await;
            continue;
        }
        if handle_remote_desktop_meta_if_present(app, registry_pending, telemetry, &chunk).await {
            continue;
        }
        if let Some(frame) = compositor
            .handle_record(&chunk)
            .map_err(|err| anyhow!(err))?
        {
            present_or_emit_remote_desktop_frame(app, viewport, &frame)
                .map_err(|err| anyhow!(err))?;
            if !first_frame_presented {
                first_frame_presented = true;
                emit_window(app, "relay:hello", "First frame rendered".to_string());
            }
        }
        if let Some(dimensions) = compositor.dimensions() {
            control_state.set_stream_size(Some(dimensions));
        }
        record_count = record_count.saturating_add(1);
    }
    info!(session_id = %session_id, record_count, "relay macOS display-delta stream received");
    Ok(())
}

async fn run_relay_connection(
    app: Window,
    session_id: String,
    relay_url: String,
    e2e_key_b64: String,
    control_state: ControlState,
    registry_pending: RegistryPendingState,
    telemetry: ConnectionTelemetryState,
    viewport: ViewportArc,
    control_rx: mpsc::UnboundedReceiver<Vec<u8>>,
    stream_protocol: String,
) -> Result<(), anyhow::Error> {
    let relay_target = parse_relay_target(&relay_url)?;
    let addr = format!("{}:{}", relay_target.host, relay_target.port);
    let connect_timeout = relay_connect_timeout();
    let connect_started_at = Instant::now();
    let tcp_stream = timeout(connect_timeout, TcpStream::connect(addr.clone()))
        .await
        .map_err(|_| anyhow!("connect relay tcp timed out"))?
        .context("connect relay tcp")?;
    let relay_tcp_ms = connect_started_at
        .elapsed()
        .as_millis()
        .min(u128::from(u64::MAX)) as u64;
    tcp_stream
        .set_nodelay(true)
        .context("set relay TCP_NODELAY")?;

    let tls_started_at = Instant::now();
    let tls_config = build_tls_config()?;
    let connector = TlsConnector::from(Arc::new(tls_config));
    let server_name =
        ServerName::try_from(relay_target.host.clone()).context("build relay server name")?;
    let mut stream = timeout(connect_timeout, connector.connect(server_name, tcp_stream))
        .await
        .map_err(|_| anyhow!("relay tls connect timed out"))?
        .context("relay tls connect")?;
    let relay_tls_ms = tls_started_at
        .elapsed()
        .as_millis()
        .min(u128::from(u64::MAX)) as u64;
    info!(session_id = %session_id, "relay TLS connected");

    let handshake_started_at = Instant::now();
    let request = format!(
        "GET /relay/{session_id} HTTP/1.1\r\nHost: {host}\r\n\r\n",
        session_id = session_id,
        host = relay_target.host
    );
    stream
        .write_all(request.as_bytes())
        .await
        .context("write relay request")?;
    info!(session_id = %session_id, "relay GET request sent");
    timeout(connect_timeout, read_http_response(&mut stream))
        .await
        .map_err(|_| anyhow!("read relay response timed out"))??;
    info!(session_id = %session_id, "relay HTTP response received");

    let key_bytes = BASE64_STANDARD
        .decode(e2e_key_b64.trim())
        .context("decode relay e2e key")?;
    let cipher = build_e2e_cipher(&key_bytes)?;

    let mut send_counter = 0u64;
    write_e2e_frame(&mut stream, &cipher, &mut send_counter, b"hello-world").await?;
    info!(session_id = %session_id, "relay hello-world sent, waiting for first frame");

    let (mut reader, writer) = tokio::io::split(stream);
    let heartbeat_cipher = build_e2e_cipher(&key_bytes)?;
    let heartbeat_interval = Duration::from_secs(15);
    tokio::spawn(run_relay_writer(
        writer,
        heartbeat_cipher,
        heartbeat_interval,
        control_rx,
    ));

    let hello_payload = read_e2e_frame_from(&mut reader, &cipher).await?;
    let message = String::from_utf8_lossy(&hello_payload).trim().to_string();
    info!(payload = %message, "relay transport hello received");
    start_connection_telemetry(
        app.clone(),
        telemetry.clone(),
        control_state.clone(),
        ConnectionStatePayload {
            session_kind: "remote_desktop".to_string(),
            transport: "relay".to_string(),
            connection_type: "relay".to_string(),
            encryption_label: "TLS + E2E ChaCha20-Poly1305".to_string(),
            encryption_details: Some(
                "Relay traffic is protected by TLS to the relay and encrypted end-to-end with ChaCha20-Poly1305."
                    .to_string(),
            ),
            remote_addr: Some(addr.clone()),
            viewer_reflex: None,
            agent_reflex: None,
            agent_local_addrs: Vec::new(),
            connect_ms: Some(connect_started_at.elapsed().as_millis().min(u128::from(u64::MAX)) as u64),
            relay_tcp_ms: Some(relay_tcp_ms),
            relay_tls_ms: Some(relay_tls_ms),
            relay_handshake_ms: Some(
                handshake_started_at
                    .elapsed()
                    .as_millis()
                    .min(u128::from(u64::MAX)) as u64,
            ),
            capture_type: None,
        },
    );
    emit_window(
        &app,
        "relay:hello",
        "Relay connected; waiting for first frame".to_string(),
    );

    let stream_protocol = stream_protocol.trim();
    if stream_protocol == REMOTE_DESKTOP_PROTOCOL_MODERN_DISPLAY_DELTA
        || stream_protocol == REMOTE_DESKTOP_PROTOCOL_EXPERIMENTAL_DISPLAY_DELTA
    {
        return run_relay_modern_display_delta(
            &app,
            &session_id,
            &mut reader,
            &cipher,
            &registry_pending,
            &telemetry,
            &viewport,
            &control_state,
        )
        .await;
    }

    // Keep reading until we see the IVF header (relay or agent may send metadata first).
    let ivf_header = loop {
        let payload = read_e2e_frame_from(&mut reader, &cipher).await?;
        if let Some(meta) = extract_rmmd_meta(&payload) {
            let _ =
                handle_remote_desktop_meta_if_present(&app, &registry_pending, &telemetry, meta)
                    .await;
            continue;
        }
        if handle_remote_desktop_meta_if_present(&app, &registry_pending, &telemetry, &payload)
            .await
        {
            continue;
        }
        if payload.len() == 32 && payload.get(0..4).map(|s| s == b"DKIF").unwrap_or(false) {
            break payload;
        }
        emit_window(
            &app,
            "relay:frame",
            String::from_utf8_lossy(&payload).to_string(),
        );
    };

    {
        let (mut stream_width, mut stream_height, stream_fps) =
            parse_ivf_header(&ivf_header).ok_or_else(|| anyhow!("invalid IVF header"))?;
        control_state.set_stream_size(Some((stream_width, stream_height)));
        let mut decoder = Vp8Decoder::new().map_err(|err| anyhow!(err))?;
        debug!(
            session_id = %session_id,
            stream_width,
            stream_height,
            stream_fps,
            "viewer relay IVF stream header parsed; VP8 decoder initialized"
        );
        let mut frame_count: u64 = 0;
        let mut first_frame_presented = false;
        loop {
            let chunk = match read_e2e_frame_from(&mut reader, &cipher).await {
                Ok(c) => c,
                Err(e) => {
                    let s = e.to_string();
                    if s.contains("relay connection closed")
                        || s.contains("UnexpectedEof")
                        || s.contains("ConnectionReset")
                        || s.contains("BrokenPipe")
                        || s.contains("ConnectionAborted")
                    {
                        break;
                    }
                    return Err(e);
                }
            };
            if let Some(meta) = extract_rmmd_meta(&chunk) {
                let _ = handle_remote_desktop_meta_if_present(
                    &app,
                    &registry_pending,
                    &telemetry,
                    meta,
                )
                .await;
                continue;
            }
            if handle_remote_desktop_meta_if_present(&app, &registry_pending, &telemetry, &chunk)
                .await
            {
                continue;
            }
            #[cfg(windows)]
            {
                if chunk.len() >= 12 {
                    let mut len_bytes = [0u8; 4];
                    len_bytes.copy_from_slice(&chunk[0..4]);
                    let payload_len = match parse_legacy_vp8_payload_prefix(len_bytes) {
                        LegacyVp8PayloadPrefix::Payload(payload_len) => payload_len,
                        LegacyVp8PayloadPrefix::MidstreamIvfHeader => {
                            warn!(
                                session_id = %session_id,
                                frame_index = frame_count,
                                "viewer relay IVF stream received unexpected midstream DKIF header"
                            );
                            break;
                        }
                        LegacyVp8PayloadPrefix::TooLarge(payload_len) => {
                            warn!(
                                session_id = %session_id,
                                frame_index = frame_count,
                                payload_len,
                                max_payload_len = MAX_LEGACY_VP8_PAYLOAD_LEN,
                                "viewer relay IVF stream rejected oversized VP8 payload"
                            );
                            break;
                        }
                    };
                    if chunk.len() >= 12 + payload_len {
                        let payload = &chunk[12..12 + payload_len];
                        if viewport_recently_moved(&viewport, 50).is_some() {
                            debug!(
                                session_id = %session_id,
                                frame_index = frame_count,
                                payload_len,
                                "viewer relay VP8 frame skipped after viewport move"
                            );
                            frame_count += 1;
                            continue;
                        }
                        debug!(
                            session_id = %session_id,
                            frame_index = frame_count,
                            payload_len,
                            chunk_len = chunk.len(),
                            "viewer relay VP8 decode input"
                        );
                        match decoder.decode(payload) {
                            Ok(Some(frame)) => {
                                let mut frame = frame;
                                frame.fps = stream_fps;
                                let frame_width = frame.width;
                                let frame_height = frame.height;
                                debug!(
                                    session_id = %session_id,
                                    frame_index = frame_count,
                                    frame_width,
                                    frame_height,
                                    "viewer relay VP8 decode produced frame"
                                );
                                let session_id_for_present = session_id.clone();
                                let frame_index_for_present = frame_count;
                                let viewport = viewport.clone();
                                let _ = app.run_on_main_thread(move || {
                                    let Ok(mut guard) = viewport.lock() else {
                                        warn!(
                                            session_id = %session_id_for_present,
                                            frame_index = frame_index_for_present,
                                            "viewer relay present skipped; viewport lock poisoned"
                                        );
                                        return;
                                    };
                                    debug!(
                                        session_id = %session_id_for_present,
                                        frame_index = frame_index_for_present,
                                        has_last_rect = guard.last_rect.is_some(),
                                        has_surface = guard.surface.is_some(),
                                        has_gpu_viewport = guard.gpu_viewport.is_some(),
                                        "viewer relay present dispatch on main thread"
                                    );
                                    if let Err(err) = present_decoded_frame(&mut guard, &frame) {
                                        warn!(
                                            session_id = %session_id_for_present,
                                            frame_index = frame_index_for_present,
                                            error = %err,
                                            "viewer relay viewport present failed"
                                        );
                                    } else {
                                        debug!(
                                            session_id = %session_id_for_present,
                                            frame_index = frame_index_for_present,
                                            "viewer relay viewport present returned"
                                        );
                                    }
                                });
                                if !first_frame_presented {
                                    first_frame_presented = true;
                                    emit_window(
                                        &app,
                                        "relay:hello",
                                        "First frame rendered".to_string(),
                                    );
                                }
                                if frame_width != stream_width || frame_height != stream_height {
                                    stream_width = frame_width;
                                    stream_height = frame_height;
                                    control_state
                                        .set_stream_size(Some((stream_width, stream_height)));
                                }
                            }
                            Ok(None) => {
                                debug!(
                                    session_id = %session_id,
                                    frame_index = frame_count,
                                    payload_len,
                                    "viewer relay VP8 decode needs more input"
                                );
                            }
                            Err(err) => {
                                warn!(
                                    session_id = %session_id,
                                    frame_index = frame_count,
                                    payload_len,
                                    error = %err,
                                    "viewer relay VP8 decode failed"
                                );
                            }
                        }
                    }
                }
            }
            #[cfg(not(windows))]
            {
                if chunk.len() >= 12 {
                    let mut len_bytes = [0u8; 4];
                    len_bytes.copy_from_slice(&chunk[0..4]);
                    let payload_len = match parse_legacy_vp8_payload_prefix(len_bytes) {
                        LegacyVp8PayloadPrefix::Payload(payload_len) => payload_len,
                        LegacyVp8PayloadPrefix::MidstreamIvfHeader => {
                            warn!(
                                session_id = %session_id,
                                frame_index = frame_count,
                                "viewer relay IVF stream received unexpected midstream DKIF header"
                            );
                            break;
                        }
                        LegacyVp8PayloadPrefix::TooLarge(payload_len) => {
                            warn!(
                                session_id = %session_id,
                                frame_index = frame_count,
                                payload_len,
                                max_payload_len = MAX_LEGACY_VP8_PAYLOAD_LEN,
                                "viewer relay IVF stream rejected oversized VP8 payload"
                            );
                            break;
                        }
                    };
                    if chunk.len() >= 12 + payload_len {
                        let payload = &chunk[12..12 + payload_len];
                        debug!(
                            session_id = %session_id,
                            frame_index = frame_count,
                            payload_len,
                            chunk_len = chunk.len(),
                            "viewer relay VP8 decode input"
                        );
                        match decoder.decode(payload) {
                            Ok(Some(mut frame)) => {
                                frame.fps = stream_fps;
                                let frame_width = frame.width;
                                let frame_height = frame.height;
                                if let Err(err) =
                                    present_or_emit_remote_desktop_frame(&app, &viewport, &frame)
                                {
                                    warn!(
                                        session_id = %session_id,
                                        frame_index = frame_count,
                                        error = %err,
                                        "viewer relay frame present failed"
                                    );
                                } else if !first_frame_presented {
                                    first_frame_presented = true;
                                    emit_window(
                                        &app,
                                        "relay:hello",
                                        "First frame rendered".to_string(),
                                    );
                                }
                                if frame_width != stream_width || frame_height != stream_height {
                                    stream_width = frame_width;
                                    stream_height = frame_height;
                                    control_state
                                        .set_stream_size(Some((stream_width, stream_height)));
                                }
                            }
                            Ok(None) => {}
                            Err(err) => {
                                warn!(
                                    session_id = %session_id,
                                    frame_index = frame_count,
                                    payload_len,
                                    error = %err,
                                    "viewer relay VP8 decode failed"
                                );
                            }
                        }
                    }
                }
            }
            frame_count += 1;
        }
        info!(session_id = %session_id, frame_count, "relay IVF stream received");
    }
    Ok(())
}

async fn run_registry_relay_connection(
    app: Window,
    session_id: String,
    relay_url: String,
    e2e_key_b64: String,
    telemetry: ConnectionTelemetryState,
    control_state: RegistryControlState,
    registry_pending: RemoteRegistryPendingState,
    control_rx: mpsc::UnboundedReceiver<Vec<u8>>,
) -> Result<(), anyhow::Error> {
    let relay_target = parse_relay_target(&relay_url)?;
    let addr = format!("{}:{}", relay_target.host, relay_target.port);
    let connect_timeout = relay_connect_timeout();
    let tcp_stream = timeout(connect_timeout, TcpStream::connect(addr.clone()))
        .await
        .map_err(|_| anyhow!("connect relay tcp timed out"))?
        .context("connect relay tcp")?;
    tcp_stream
        .set_nodelay(true)
        .context("set relay TCP_NODELAY")?;

    let tls_config = build_tls_config()?;
    let connector = TlsConnector::from(Arc::new(tls_config));
    let server_name =
        ServerName::try_from(relay_target.host.clone()).context("build relay server name")?;
    let mut stream = timeout(connect_timeout, connector.connect(server_name, tcp_stream))
        .await
        .map_err(|_| anyhow!("relay tls connect timed out"))?
        .context("relay tls connect")?;

    let request = format!(
        "GET /relay/{session_id} HTTP/1.1\r\nHost: {host}\r\n\r\n",
        session_id = session_id,
        host = relay_target.host
    );
    stream
        .write_all(request.as_bytes())
        .await
        .context("write relay request")?;
    timeout(connect_timeout, read_http_response(&mut stream))
        .await
        .map_err(|_| anyhow!("read relay response timed out"))??;

    let key_bytes = BASE64_STANDARD
        .decode(e2e_key_b64.trim())
        .context("decode relay e2e key")?;
    let cipher = build_e2e_cipher(&key_bytes)?;

    let mut send_counter = 0u64;
    write_e2e_frame(&mut stream, &cipher, &mut send_counter, b"hello-world").await?;

    let (mut reader, writer) = tokio::io::split(stream);
    let heartbeat_cipher = build_e2e_cipher(&key_bytes)?;
    let heartbeat_interval = Duration::from_secs(15);
    tokio::spawn(run_relay_writer(
        writer,
        heartbeat_cipher,
        heartbeat_interval,
        control_rx,
    ));

    // First frame is a hello string.
    let hello_payload = read_e2e_frame_from(&mut reader, &cipher).await?;
    let message = String::from_utf8_lossy(&hello_payload).trim().to_string();
    emit_window(&app, "registry:relay:hello", message);
    start_connection_telemetry(
        app.clone(),
        telemetry.clone(),
        ControlState::default(),
        ConnectionStatePayload {
            session_kind: "remote_registry".to_string(),
            transport: "relay".to_string(),
            connection_type: "relay".to_string(),
            encryption_label: "TLS + E2E ChaCha20-Poly1305".to_string(),
            encryption_details: Some(
                "Registry relay traffic is protected by TLS to the relay and encrypted end-to-end with ChaCha20-Poly1305."
                    .to_string(),
            ),
            remote_addr: Some(addr.clone()),
            viewer_reflex: None,
            agent_reflex: None,
            agent_local_addrs: Vec::new(),
            connect_ms: None,
            relay_tcp_ms: None,
            relay_tls_ms: None,
            relay_handshake_ms: None,
            capture_type: None,
        },
    );

    loop {
        let payload = match read_e2e_frame_from(&mut reader, &cipher).await {
            Ok(p) => p,
            Err(_) => break,
        };
        if let Some(meta) = extract_rmmd_meta(&payload) {
            let _ =
                handle_remote_registry_response_if_present(&control_state, &registry_pending, meta)
                    .await;
            continue;
        }
        let _ =
            handle_remote_registry_response_if_present(&control_state, &registry_pending, &payload)
                .await;
    }

    Ok(())
}

/// Sends heartbeat and control frames over the relay.
/// Uses its own nonce counter starting at 1 (0 was used for hello-world).
async fn run_relay_writer<W>(
    mut writer: W,
    cipher: ChaCha20Poly1305,
    interval_duration: Duration,
    mut control_rx: mpsc::UnboundedReceiver<Vec<u8>>,
) where
    W: AsyncWriteExt + Unpin + Send,
{
    let mut send_counter = 1u64; // 0 was used for hello-world
    let mut ticker = interval(interval_duration);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut control_closed = false;
    loop {
        tokio::select! {
            _ = ticker.tick() => {
                if let Err(e) =
                    write_e2e_frame(&mut writer, &cipher, &mut send_counter, HEARTBEAT_PAYLOAD).await
                {
                    warn!(error = %e, "heartbeat send failed, stopping");
                    break;
                }
            }
            msg = control_rx.recv(), if !control_closed => {
                match msg {
                    Some(frame) => {
                        if let Err(e) = write_e2e_frame(&mut writer, &cipher, &mut send_counter, &frame).await {
                            warn!(error = %e, "control send failed, stopping");
                            break;
                        }
                    }
                    None => {
                        control_closed = true;
                    }
                }
            }
        }
    }
}

pub(crate) fn build_client_config(
    psk_cert_pem: &str,
) -> Result<quinn::ClientConfig, anyhow::Error> {
    let mut roots = rustls::RootCertStore::empty();
    let mut reader = std::io::BufReader::new(psk_cert_pem.as_bytes());
    let certs = certs(&mut reader)
        .collect::<std::io::Result<Vec<_>>>()
        .context("read cert pem")?;
    for cert in certs {
        roots.add(cert).context("add cert to root store")?;
    }

    let rustls_config = rustls::ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    let quic_config = QuicClientConfig::try_from(rustls_config).context("build quic client")?;
    let mut client_config = quinn::ClientConfig::new(Arc::new(quic_config));

    let mut transport = quinn::TransportConfig::default();
    transport.keep_alive_interval(Some(Duration::from_secs(4)));
    let idle_timeout = quinn::IdleTimeout::try_from(Duration::from_secs(180))
        .map_err(|_| anyhow!("build quic idle timeout"))?;
    transport.max_idle_timeout(Some(idle_timeout));
    client_config.transport_config(Arc::new(transport));

    Ok(client_config)
}

// ---------------------------------------------------------------------------
// File transfer transport (viewer ↔ agent, QUIC or relay)
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct FileTransferConnectionState(pub Arc<AsyncMutex<Option<FileTransferConnection>>>);

impl Default for FileTransferConnectionState {
    fn default() -> Self {
        Self(Arc::new(AsyncMutex::new(None)))
    }
}

#[derive(Clone, Default)]
struct FileTransferCancelState(pub Arc<AsyncMutex<HashMap<String, CancellationToken>>>);

#[derive(Clone, Default)]
struct FileTransferOperationGateState(pub Arc<AsyncMutex<()>>);

const FILE_TRANSFER_CANCELLED_MESSAGE: &str = "cancelled";
const FILE_TRANSFER_PREPARE_TIMEOUT: Duration = Duration::from_secs(120);
const FILE_TRANSFER_RESPONSE_TIMEOUT: Duration = Duration::from_secs(30);

/// Best-effort cleanup for temp transfer artifacts.
///
/// This is intentionally simple (no extra deps): if a transfer errors, is cancelled, or returns a
/// conflict response without persisting the file, we don't want `rmm_*_*.zip` / `.bin` to pile up
/// in the OS temp directory.
struct RemoveFileOnDrop(Option<PathBuf>);

impl RemoveFileOnDrop {
    fn new(path: PathBuf) -> Self {
        Self(Some(path))
    }

    fn disarm(&mut self) {
        self.0 = None;
    }
}

fn file_transfer_error_response(
    code: OperationErrorCode,
    message: impl Into<String>,
    retryable: bool,
) -> FileTransferResponse {
    FileTransferResponse::Error {
        code,
        message: message.into(),
        retryable,
    }
}

fn file_transfer_cancelled_response() -> FileTransferResponse {
    file_transfer_error_response(
        OperationErrorCode::Cancelled,
        FILE_TRANSFER_CANCELLED_MESSAGE,
        true,
    )
}

fn file_transfer_request_transfer_id(request: &FileTransferRequest) -> Option<&str> {
    match request {
        FileTransferRequest::Download { transfer_id, .. }
        | FileTransferRequest::Upload { transfer_id, .. }
        | FileTransferRequest::Cancel { transfer_id } => Some(transfer_id.as_str()),
        FileTransferRequest::ListDir { .. }
        | FileTransferRequest::Rename { .. }
        | FileTransferRequest::Delete { .. } => None,
    }
}

fn build_resumable_download_temp_path(transfer_id: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    let safe_transfer_id = transfer_id
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>();
    path.push(format!("talos_viewer_download_{safe_transfer_id}.part"));
    path
}

fn file_size_if_exists(path: &Path) -> Result<u64, anyhow::Error> {
    match fs::metadata(path) {
        Ok(metadata) => Ok(metadata.len()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(0),
        Err(error) => Err(error).context("read file metadata"),
    }
}

impl Drop for RemoveFileOnDrop {
    fn drop(&mut self) {
        if let Some(path) = self.0.take() {
            let _ = fs::remove_file(path);
        }
    }
}

enum FileTransferConnection {
    Quic {
        _endpoint: Endpoint,
        connection: Connection,
    },
    Relay(RelayFileTransferConnection),
}

struct RelayFileTransferConnection {
    reader: tokio::io::ReadHalf<tokio_rustls::client::TlsStream<TcpStream>>,
    writer: tokio::io::WriteHalf<tokio_rustls::client::TlsStream<TcpStream>>,
    cipher: ChaCha20Poly1305,
    send_counter: u64,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct FileTransferProgressPayload {
    job_id: String,
    direction: String,
    file_name: String,
    bytes_done: u64,
    bytes_total: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    phase: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
}

#[derive(Clone, Copy)]
struct UploadPreparationProgress {
    files_done: usize,
    files_total: usize,
    bytes_done: u64,
    bytes_total: u64,
}

struct UploadBundle {
    source_path: PathBuf,
    file_name: String,
    size_bytes: u64,
    is_archive: bool,
    extract_archive: bool,
    cleanup_source: bool,
}

#[derive(Debug)]
enum LocalTransferError {
    Message(String),
    Conflict { path: String, message: String },
}

impl std::fmt::Display for LocalTransferError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LocalTransferError::Message(message) => write!(f, "{message}"),
            LocalTransferError::Conflict { path, message } => {
                write!(f, "conflict at {path}: {message}")
            }
        }
    }
}

impl std::error::Error for LocalTransferError {}

impl From<std::io::Error> for LocalTransferError {
    fn from(value: std::io::Error) -> Self {
        LocalTransferError::Message(local_io_error_message(&value))
    }
}

impl From<walkdir::Error> for LocalTransferError {
    fn from(value: walkdir::Error) -> Self {
        LocalTransferError::Message(value.to_string())
    }
}

impl From<zip::result::ZipError> for LocalTransferError {
    fn from(value: zip::result::ZipError) -> Self {
        LocalTransferError::Message(value.to_string())
    }
}

fn local_io_error_message(error: &std::io::Error) -> String {
    #[cfg(target_os = "macos")]
    if error.kind() == std::io::ErrorKind::PermissionDenied {
        return "macOS denied access to this folder. Grant Full Disk Access to Talos Viewer in System Settings, then refresh the folder.".to_string();
    }

    error.to_string()
}

#[tauri::command]
async fn file_transfer_connect(
    window: Window,
    window_states: State<'_, AppWindowStates>,
    session_id: String,
    token: String,
    api_base: String,
    viewer_transport: String,
    transports: Vec<String>,
    agent_reflex: Option<ReflexAddress>,
    agent_host: Option<String>,
    agent_local_addrs: Option<Vec<LocalAddr>>,
    psk_cert_pem: Option<String>,
    relay_url: Option<String>,
    e2e_key: Option<String>,
    quic_timeout_ms: Option<u64>,
) -> Result<String, String> {
    let state = window_states.get_or_create(window.label());
    let ft_state = state.file_transfer.clone();
    let connect_started_at = Instant::now();
    {
        let mut guard = ft_state.0.lock().await;
        *guard = None;
    }

    let normalized_transport = viewer_transport.trim().to_ascii_lowercase();
    let supports_quic = transports.iter().any(|transport| transport == "quic");
    let supports_relay = transports.iter().any(|transport| transport == "relay");

    if normalized_transport != "tcprelay"
        && supports_quic
        && agent_reflex.is_some()
        && psk_cert_pem.is_some()
    {
        let quic_attempt = connect_file_transfer_quic_transport(
            &session_id,
            &token,
            &api_base,
            agent_reflex.as_ref().expect("checked above"),
            agent_host.clone(),
            agent_local_addrs.clone(),
            psk_cert_pem.as_ref().expect("checked above"),
            quic_timeout_ms,
        )
        .await;

        match quic_attempt {
            Ok((endpoint, connection)) => {
                let remote_addr = connection.remote_address().to_string();
                let remote_ip = connection.remote_address().ip().to_string();
                let connection_type = {
                    let viewer_addrs = viewer_local_addrs();
                    let lan_candidate = match &agent_local_addrs {
                        Some(addrs) => pick_lan_candidate(&viewer_addrs, addrs),
                        None => agent_host.clone().filter(|host| !host.trim().is_empty()),
                    };
                    if lan_candidate.as_deref() == Some(remote_ip.as_str()) {
                        "lan_direct"
                    } else {
                        "hole_punch"
                    }
                };
                let mut guard = ft_state.0.lock().await;
                *guard = Some(FileTransferConnection::Quic {
                    _endpoint: endpoint,
                    connection,
                });
                emit_connection_state(
                    &window,
                    &ConnectionStatePayload {
                        session_kind: "file_transfer".to_string(),
                        transport: "quic".to_string(),
                        connection_type: connection_type.to_string(),
                        encryption_label: "Pinned QUIC TLS".to_string(),
                        encryption_details: Some(
                            "File transfer QUIC session authenticated with the per-session pinned certificate."
                                .to_string(),
                        ),
                        remote_addr: Some(remote_addr),
                        viewer_reflex: None,
                        agent_reflex,
                        agent_local_addrs: agent_local_addrs.unwrap_or_default(),
                        connect_ms: Some(
                            connect_started_at
                                .elapsed()
                                .as_millis()
                                .min(u128::from(u64::MAX))
                                as u64,
                        ),
                        relay_tcp_ms: None,
                        relay_tls_ms: None,
                        relay_handshake_ms: None,
                        capture_type: None,
                    },
                );
                return Ok("quic".to_string());
            }
            Err(error) if normalized_transport == "quic" => {
                return Err(format!("file transfer quic connect failed: {error}"));
            }
            Err(error) => {
                warn!(error = %error, "file transfer quic failed, trying relay fallback");
            }
        }
    }

    if supports_relay {
        let relay_url = relay_url.ok_or_else(|| "relay url missing".to_string())?;
        let e2e_key = e2e_key.ok_or_else(|| "relay e2e key missing".to_string())?;
        request_file_transfer_relay(&api_base, &session_id, &token)
            .await
            .map_err(|error| format!("request file transfer relay: {error}"))?;
        let relay_connection =
            connect_file_transfer_relay_transport(&session_id, &relay_url, &e2e_key)
                .await
                .map_err(|error| format!("file transfer relay connect failed: {error}"))?;
        let remote_addr = parse_relay_target(&relay_url)
            .map(|target| format!("{}:{}", target.host, target.port))
            .unwrap_or_else(|_| relay_url.clone());
        let mut guard = ft_state.0.lock().await;
        *guard = Some(FileTransferConnection::Relay(relay_connection));
        emit_connection_state(
            &window,
            &ConnectionStatePayload {
                session_kind: "file_transfer".to_string(),
                transport: "relay".to_string(),
                connection_type: "relay".to_string(),
                encryption_label: "TLS + E2E ChaCha20-Poly1305".to_string(),
                encryption_details: Some(
                    "File transfer relay traffic is protected by TLS to the relay and encrypted end-to-end with ChaCha20-Poly1305."
                        .to_string(),
                ),
                remote_addr: Some(remote_addr),
                viewer_reflex: None,
                agent_reflex,
                agent_local_addrs: agent_local_addrs.unwrap_or_default(),
                connect_ms: Some(
                    connect_started_at
                        .elapsed()
                        .as_millis()
                        .min(u128::from(u64::MAX))
                        as u64,
                ),
                relay_tcp_ms: None,
                relay_tls_ms: None,
                relay_handshake_ms: None,
                capture_type: None,
            },
        );
        return Ok("relay".to_string());
    }

    Err("no compatible file transfer transport available".to_string())
}

#[tauri::command]
async fn file_transfer_disconnect(
    window: Window,
    window_states: State<'_, AppWindowStates>,
) -> Result<(), String> {
    let state = window_states.get_or_create(window.label());
    let mut guard = state.file_transfer.0.lock().await;
    *guard = None;
    Ok(())
}

#[tauri::command]
async fn file_transfer_cancel(
    window: Window,
    window_states: State<'_, AppWindowStates>,
    job_id: String,
) -> Result<(), String> {
    let state = window_states.get_or_create(window.label());
    let guard = state.file_transfer_cancel.0.lock().await;
    if let Some(token) = guard.get(job_id.trim()) {
        token.cancel();
        return Ok(());
    }
    Err("transfer job not found".to_string())
}

#[tauri::command]
async fn file_transfer_list_local(path: Option<String>) -> Result<FileTransferResponse, String> {
    list_local_dir(path.as_deref().unwrap_or("/")).map_err(|error| error.to_string())
}

#[tauri::command]
async fn file_transfer_list_remote(
    window: Window,
    window_states: State<'_, AppWindowStates>,
    path: String,
) -> Result<FileTransferResponse, String> {
    let state = window_states.get_or_create(window.label());
    let mut guard = state.file_transfer.0.lock().await;
    let Some(connection) = guard.as_mut() else {
        return Err("file transfer not connected".to_string());
    };
    let request = FileTransferRequest::ListDir { path };
    request_file_transfer_json(connection, &request)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn file_transfer_remote_rename(
    window: Window,
    window_states: State<'_, AppWindowStates>,
    from_path: String,
    to_path: String,
) -> Result<(), String> {
    let state = window_states.get_or_create(window.label());
    let mut guard = state.file_transfer.0.lock().await;
    let Some(connection) = guard.as_mut() else {
        return Err("file transfer not connected".to_string());
    };
    let request = FileTransferRequest::Rename { from_path, to_path };
    let response = request_file_transfer_json(connection, &request)
        .await
        .map_err(|error| error.to_string())?;
    match response {
        FileTransferResponse::Ok { .. } => Ok(()),
        FileTransferResponse::Error { message, .. } => Err(message),
        other => Err(format!("unexpected file transfer response: {other:?}")),
    }
}

#[tauri::command]
async fn file_transfer_remote_delete(
    window: Window,
    window_states: State<'_, AppWindowStates>,
    path: String,
    recursive: bool,
) -> Result<(), String> {
    let state = window_states.get_or_create(window.label());
    let mut guard = state.file_transfer.0.lock().await;
    let Some(connection) = guard.as_mut() else {
        return Err("file transfer not connected".to_string());
    };
    let request = FileTransferRequest::Delete { path, recursive };
    let response = request_file_transfer_json(connection, &request)
        .await
        .map_err(|error| error.to_string())?;
    match response {
        FileTransferResponse::Ok { .. } => Ok(()),
        FileTransferResponse::Error { message, .. } => Err(message),
        other => Err(format!("unexpected file transfer response: {other:?}")),
    }
}

#[tauri::command]
async fn file_transfer_local_rename(from_path: String, to_path: String) -> Result<(), String> {
    rename_local_path(&from_path, &to_path).map_err(|error| error.to_string())
}

#[tauri::command]
async fn file_transfer_local_delete(path: String, recursive: bool) -> Result<(), String> {
    delete_local_path(&path, recursive).map_err(|error| error.to_string())
}

#[tauri::command]
async fn file_transfer_upload(
    app: Window,
    window_states: State<'_, AppWindowStates>,
    job_id: String,
    local_paths: Vec<String>,
    remote_destination: String,
    conflict_mode: FileTransferConflictMode,
) -> Result<FileTransferResponse, String> {
    let window_state = window_states.get_or_create(app.label());
    let gate_state = window_state.file_transfer_gate;
    let state = window_state.file_transfer;
    let cancel_state = window_state.file_transfer_cancel;

    {
        let guard = state.0.lock().await;
        if guard.is_none() {
            return Err("file transfer not connected".to_string());
        }
    }

    let cancel_token = CancellationToken::new();
    {
        let mut guard = cancel_state.0.lock().await;
        guard.insert(job_id.clone(), cancel_token.clone());
    }

    // If another transfer is already in-flight, surface that as queued.
    if gate_state.0.try_lock().is_err() {
        emit_file_transfer_progress(
            &app,
            &job_id,
            "upload",
            "Queued",
            0,
            0,
            Some("preparing"),
            Some("Queued..."),
        );
    }

    let gate_guard = tokio::select! {
        _ = cancel_token.cancelled() => {
            let mut guard = cancel_state.0.lock().await;
            guard.remove(&job_id);
            emit_file_transfer_progress(
                &app,
                &job_id,
                "upload",
                "Cancelled",
                0,
                0,
                Some("finalizing"),
                Some("Cancelled"),
            );
            return Ok(file_transfer_cancelled_response());
        }
        guard = gate_state.0.lock() => guard,
    };

    let selection_label = if local_paths.len() == 1 {
        "1 item".to_string()
    } else {
        format!("{} items", local_paths.len())
    };
    emit_file_transfer_progress(
        &app,
        &job_id,
        "upload",
        &selection_label,
        0,
        0,
        Some("preparing"),
        Some("Scanning local selection..."),
    );

    let cancelled = Arc::new(AtomicBool::new(false));
    let cancelled_for_prepare = cancelled.clone();
    let (progress_tx, mut progress_rx) =
        tokio::sync::mpsc::unbounded_channel::<UploadPreparationProgress>();
    let local_paths_for_prepare = local_paths.clone();
    let mut prepare_task = tokio::task::spawn_blocking(move || {
        prepare_upload_bundle(
            &local_paths_for_prepare,
            cancelled_for_prepare.as_ref(),
            |progress| {
                let _ = progress_tx.send(progress);
            },
        )
    });

    let bundle = loop {
        tokio::select! {
            _ = cancel_token.cancelled() => {
                cancelled.store(true, Ordering::Relaxed);
                prepare_task.abort();
                let mut guard = cancel_state.0.lock().await;
                guard.remove(&job_id);
                emit_file_transfer_progress(
                    &app,
                    &job_id,
                    "upload",
                    "Cancelled",
                    0,
                    0,
                    Some("finalizing"),
                    Some("Cancelled"),
                );
                return Ok(file_transfer_cancelled_response());
            }
            result = &mut prepare_task => {
                match result {
                    Ok(Ok(bundle)) => break bundle,
                    Ok(Err(error)) => return Err(error.to_string()),
                    Err(error) => return Err(format!("upload preparation task failed: {error}")),
                }
            }
            _ = sleep(FILE_TRANSFER_PREPARE_TIMEOUT) => {
                cancelled.store(true, Ordering::Relaxed);
                prepare_task.abort();
                let mut guard = cancel_state.0.lock().await;
                guard.remove(&job_id);
                return Err(format!(
                    "upload preparation timed out after {} seconds",
                    FILE_TRANSFER_PREPARE_TIMEOUT.as_secs()
                ));
            }
            Some(progress) = progress_rx.recv() => {
                let phase_message = format!(
                    "Preparing archive: {} / {} file(s)",
                    progress.files_done, progress.files_total
                );
                emit_file_transfer_progress(
                    &app,
                    &job_id,
                    "upload",
                    &selection_label,
                    progress.bytes_done,
                    progress.bytes_total,
                    Some("preparing"),
                    Some(phase_message.as_str()),
                );
            }
        }
    };

    if cancel_token.is_cancelled() {
        if bundle.cleanup_source {
            let _ = fs::remove_file(&bundle.source_path);
        }
        let mut guard = cancel_state.0.lock().await;
        guard.remove(&job_id);
        emit_file_transfer_progress(
            &app,
            &job_id,
            "upload",
            &bundle.file_name,
            0,
            bundle.size_bytes,
            Some("finalizing"),
            Some("Cancelled"),
        );
        return Ok(file_transfer_cancelled_response());
    }

    emit_file_transfer_progress(
        &app,
        &job_id,
        "upload",
        &bundle.file_name,
        0,
        bundle.size_bytes,
        Some("transferring"),
        Some("Uploading to remote host..."),
    );

    let mut guard = state.0.lock().await;
    let Some(connection) = guard.as_mut() else {
        if bundle.cleanup_source {
            let _ = fs::remove_file(bundle.source_path);
        }
        return Err("file transfer not connected".to_string());
    };

    let request = FileTransferRequest::Upload {
        transfer_id: job_id.clone(),
        destination_path: remote_destination,
        file_name: bundle.file_name.clone(),
        is_archive: bundle.is_archive,
        extract_archive: bundle.extract_archive,
        conflict_mode,
        expected_size_bytes: bundle.size_bytes,
        resume_offset: 0,
    };

    let response = match connection {
        FileTransferConnection::Quic { connection, .. } => {
            upload_over_quic(
                &app,
                &job_id,
                connection,
                &cancel_token,
                &request,
                &bundle.source_path,
                &bundle.file_name,
                bundle.size_bytes,
            )
            .await
        }
        FileTransferConnection::Relay(relay) => {
            upload_over_relay(
                &app,
                &job_id,
                relay,
                &cancel_token,
                &request,
                &bundle.source_path,
                &bundle.file_name,
                bundle.size_bytes,
            )
            .await
        }
    };

    if bundle.cleanup_source {
        let _ = fs::remove_file(bundle.source_path);
    }

    let mut guard = cancel_state.0.lock().await;
    guard.remove(&job_id);

    drop(gate_guard);
    response.map_err(|error| error.to_string())
}

#[tauri::command]
async fn file_transfer_download(
    app: Window,
    window_states: State<'_, AppWindowStates>,
    job_id: String,
    remote_paths: Vec<String>,
    local_destination: String,
    conflict_mode: FileTransferConflictMode,
) -> Result<FileTransferResponse, String> {
    let window_state = window_states.get_or_create(app.label());
    let gate_state = window_state.file_transfer_gate;
    let state = window_state.file_transfer;
    let cancel_state = window_state.file_transfer_cancel;

    let destination = normalize_existing_or_creatable_dir(&local_destination)
        .map_err(|error| error.to_string())?;
    let selection_label = if remote_paths.len() == 1 {
        "1 item".to_string()
    } else {
        format!("{} items", remote_paths.len())
    };
    let cancel_token = CancellationToken::new();
    {
        let mut guard = cancel_state.0.lock().await;
        guard.insert(job_id.clone(), cancel_token.clone());
    }

    // If another transfer is already in-flight, surface that as queued.
    if gate_state.0.try_lock().is_err() {
        emit_file_transfer_progress(
            &app,
            &job_id,
            "download",
            &selection_label,
            0,
            0,
            Some("preparing"),
            Some("Queued..."),
        );
    } else {
        emit_file_transfer_progress(
            &app,
            &job_id,
            "download",
            &selection_label,
            0,
            0,
            Some("preparing"),
            Some("Preparing transfer..."),
        );
    }

    let gate_guard = tokio::select! {
        _ = cancel_token.cancelled() => {
            let mut guard = cancel_state.0.lock().await;
            guard.remove(&job_id);
            emit_file_transfer_progress(
                &app,
                &job_id,
                "download",
                "Cancelled",
                0,
                0,
                Some("finalizing"),
                Some("Cancelled"),
            );
            return Ok(file_transfer_cancelled_response());
        }
        guard = gate_state.0.lock() => guard,
    };

    emit_file_transfer_progress(
        &app,
        &job_id,
        "download",
        &selection_label,
        0,
        0,
        Some("preparing"),
        Some("Requesting remote transfer bundle..."),
    );

    let mut guard = state.0.lock().await;
    let Some(connection) = guard.as_mut() else {
        return Err("file transfer not connected".to_string());
    };
    let temp_path = build_resumable_download_temp_path(&job_id);
    let resume_offset = file_size_if_exists(&temp_path).map_err(|error| error.to_string())?;
    let request = FileTransferRequest::Download {
        transfer_id: job_id.clone(),
        paths: remote_paths,
        resume_offset,
    };

    let response = match connection {
        FileTransferConnection::Quic { connection, .. } => {
            download_over_quic(
                &app,
                &job_id,
                connection,
                &cancel_token,
                &request,
                &destination,
                &temp_path,
                conflict_mode,
            )
            .await
        }
        FileTransferConnection::Relay(relay) => {
            download_over_relay(
                &app,
                &job_id,
                relay,
                &cancel_token,
                &request,
                &destination,
                &temp_path,
                conflict_mode,
            )
            .await
        }
    };

    let mut guard = cancel_state.0.lock().await;
    guard.remove(&job_id);

    drop(gate_guard);
    response.map_err(|error| error.to_string())
}

#[tauri::command]
async fn viewer_chat_connect(
    window: Window,
    window_states: State<'_, AppWindowStates>,
    session_id: String,
    token: String,
    api_base: String,
    viewer_transport: String,
    transports: Vec<String>,
    agent_reflex: Option<ReflexAddress>,
    agent_host: Option<String>,
    agent_local_addrs: Option<Vec<LocalAddr>>,
    psk_cert_pem: Option<String>,
    relay_url: Option<String>,
    e2e_key: Option<String>,
    quic_timeout_ms: Option<u64>,
) -> Result<(), String> {
    let state = window_states.get_or_create(window.label());
    viewer_chat::chat_connect(
        window,
        state.chat.clone(),
        session_id,
        token,
        api_base,
        viewer_transport,
        transports,
        agent_reflex,
        agent_host,
        agent_local_addrs,
        psk_cert_pem,
        relay_url,
        e2e_key,
        quic_timeout_ms,
    )
    .await
}

#[tauri::command]
async fn viewer_chat_send(
    window: Window,
    window_states: State<'_, AppWindowStates>,
    text: String,
) -> Result<serde_json::Value, String> {
    let state = window_states.get_or_create(window.label());
    viewer_chat::chat_send_message(state.chat.clone(), text).await
}

#[tauri::command]
async fn viewer_chat_disconnect(
    window: Window,
    window_states: State<'_, AppWindowStates>,
    api_base: String,
    session_id: String,
    token: String,
) -> Result<(), String> {
    let state = window_states.get_or_create(window.label());
    viewer_chat::chat_disconnect(window, state.chat.clone(), api_base, session_id, token).await;
    Ok(())
}

async fn connect_file_transfer_quic_transport(
    session_id: &str,
    token: &str,
    api_base: &str,
    agent_reflex: &ReflexAddress,
    agent_host: Option<String>,
    agent_local_addrs: Option<Vec<LocalAddr>>,
    psk_cert_pem: &str,
    quic_timeout_ms: Option<u64>,
) -> Result<(Endpoint, Connection), anyhow::Error> {
    let viewer_addrs = viewer_local_addrs();
    let lan_candidate = match &agent_local_addrs {
        Some(addrs) => pick_lan_candidate(&viewer_addrs, addrs),
        None => agent_host.filter(|host| !host.trim().is_empty()),
    };
    let reflex_addr: SocketAddr = format!("{}:{}", agent_reflex.ip, agent_reflex.port)
        .parse()
        .context("parse agent reflex address")?;
    let lan_addr = lan_candidate
        .as_ref()
        .map(|ip| format!("{}:{}", ip, agent_reflex.port).parse::<SocketAddr>())
        .transpose()
        .context("parse lan candidate")?;

    let socket = UdpSocket::bind("0.0.0.0:0").context("bind quic socket")?;
    socket
        .set_nonblocking(true)
        .context("set quic socket nonblocking")?;

    let viewer_reflex = tokio::task::spawn_blocking({
        let stun_socket = socket.try_clone().ok();
        move || -> Result<SocketAddr, anyhow::Error> {
            let stun_socket = stun_socket.ok_or_else(|| anyhow!("stun socket clone failed"))?;
            query_configured_stun_reflex(stun_socket)
        }
    })
    .await
    .context("join stun task")??;

    let reflex_url = format!(
        "{}/api/rmm/file-transfer/session/{}/viewer-reflex?token={}",
        api_base.trim_end_matches('/'),
        session_id,
        urlencoding::encode(token)
    );
    let reflex_body = serde_json::json!({
        "ip": viewer_reflex.ip().to_string(),
        "port": viewer_reflex.port(),
    });
    let reflex_response = Client::new()
        .post(reflex_url)
        .json(&reflex_body)
        .send()
        .await
        .context("post file transfer viewer reflex")?;
    if !reflex_response.status().is_success() {
        return Err(anyhow!(
            "file transfer viewer reflex failed ({})",
            reflex_response.status()
        ));
    }

    let mut endpoint = Endpoint::new(
        EndpointConfig::default(),
        None,
        socket,
        Arc::new(TokioRuntime),
    )
    .context("create quic endpoint")?;
    let client_config = build_client_config(psk_cert_pem)?;
    endpoint.set_default_client_config(client_config);

    let quic_timeout = Duration::from_millis(quic_timeout_ms.unwrap_or(500));
    let connection = if let Some(lan_addr) = lan_addr {
        let mut lan_handle = tokio::spawn(run_quic_with_timeout(
            endpoint.clone(),
            session_id.to_string(),
            lan_addr,
            quic_timeout,
        ));
        let mut reflex_handle = tokio::spawn(run_quic_with_timeout(
            endpoint.clone(),
            session_id.to_string(),
            reflex_addr,
            quic_timeout,
        ));

        let mut errors: Vec<anyhow::Error> = Vec::new();
        let mut lan_done = false;
        let mut reflex_done = false;
        loop {
            tokio::select! {
                result = &mut lan_handle, if !lan_done => {
                    match result {
                        Ok(Ok(connection)) => {
                            reflex_handle.abort();
                            break connection;
                        }
                        Ok(Err(error)) => {
                            errors.push(error);
                            lan_done = true;
                        }
                        Err(error) => {
                            errors.push(anyhow!("lan connect task: {error}"));
                            lan_done = true;
                        }
                    }
                }
                result = &mut reflex_handle, if !reflex_done => {
                    match result {
                        Ok(Ok(connection)) => {
                            lan_handle.abort();
                            break connection;
                        }
                        Ok(Err(error)) => {
                            errors.push(error);
                            reflex_done = true;
                        }
                        Err(error) => {
                            errors.push(anyhow!("reflex connect task: {error}"));
                            reflex_done = true;
                        }
                    }
                }
            }

            if lan_done && reflex_done {
                return Err(errors
                    .pop()
                    .unwrap_or_else(|| anyhow!("quic connect failed")));
            }
        }
    } else {
        run_quic_with_timeout(
            endpoint.clone(),
            session_id.to_string(),
            reflex_addr,
            quic_timeout,
        )
        .await?
    };

    Ok((endpoint, connection))
}

async fn request_file_transfer_relay(
    api_base: &str,
    session_id: &str,
    token: &str,
) -> Result<(), anyhow::Error> {
    let url = format!(
        "{}/api/rmm/file-transfer/session/{}/request-relay?token={}",
        api_base.trim_end_matches('/'),
        session_id,
        urlencoding::encode(token)
    );
    let response = Client::new()
        .post(url)
        .send()
        .await
        .context("request file transfer relay")?;
    if !response.status().is_success() {
        return Err(anyhow!("request relay failed ({})", response.status()));
    }
    Ok(())
}

async fn connect_file_transfer_relay_transport(
    session_id: &str,
    relay_url: &str,
    e2e_key: &str,
) -> Result<RelayFileTransferConnection, anyhow::Error> {
    let relay_target = parse_relay_target(relay_url)?;
    let addr = format!("{}:{}", relay_target.host, relay_target.port);
    let connect_timeout = relay_connect_timeout();
    let tcp_stream = timeout(connect_timeout, TcpStream::connect(addr.clone()))
        .await
        .map_err(|_| anyhow!("connect relay tcp timed out"))?
        .context("connect relay tcp")?;
    tcp_stream
        .set_nodelay(true)
        .context("set relay tcp_nodelay")?;

    let tls_config = build_tls_config()?;
    let connector = TlsConnector::from(Arc::new(tls_config));
    let server_name =
        ServerName::try_from(relay_target.host.clone()).context("build relay server name")?;
    let mut stream = timeout(connect_timeout, connector.connect(server_name, tcp_stream))
        .await
        .map_err(|_| anyhow!("relay tls connect timed out"))?
        .context("relay tls connect")?;

    let request = format!(
        "GET /relay/{session_id} HTTP/1.1\r\nHost: {host}\r\n\r\n",
        session_id = session_id,
        host = relay_target.host
    );
    stream
        .write_all(request.as_bytes())
        .await
        .context("write relay request")?;
    timeout(connect_timeout, read_http_response(&mut stream))
        .await
        .map_err(|_| anyhow!("read relay response timed out"))??;

    let key_bytes = BASE64_URL_SAFE_NO_PAD
        .decode(e2e_key.trim())
        .or_else(|_| BASE64_STANDARD.decode(e2e_key.trim()))
        .context("decode relay e2e key")?;
    let cipher = build_e2e_cipher(&key_bytes)?;

    let mut send_counter = 0u64;
    write_e2e_frame(&mut stream, &cipher, &mut send_counter, b"hello-world")
        .await
        .context("send relay hello")?;

    let (reader, writer) = tokio::io::split(stream);
    Ok(RelayFileTransferConnection {
        reader,
        writer,
        cipher,
        send_counter,
    })
}

async fn request_file_transfer_json(
    connection: &mut FileTransferConnection,
    request: &FileTransferRequest,
) -> Result<FileTransferResponse, anyhow::Error> {
    match connection {
        FileTransferConnection::Quic { connection, .. } => {
            let (mut send, mut recv) = connection.open_bi().await.map_err(|error| {
                let close_reason = connection
                    .close_reason()
                    .map(|reason| reason.to_string())
                    .unwrap_or_else(|| "none".to_string());
                anyhow!("open file transfer quic stream: {error} (close reason: {close_reason})")
            })?;
            write_file_transfer_json_quic_frame(&mut send, request).await?;
            let _ = send.finish();
            let Some((message_type, payload)) = read_file_transfer_quic_frame(&mut recv).await?
            else {
                return Err(anyhow!("file transfer response stream closed"));
            };
            parse_file_transfer_json_response(message_type, &payload)
        }
        FileTransferConnection::Relay(relay) => {
            write_file_transfer_json_relay_frame(relay, request).await?;
            let (message_type, payload) = read_file_transfer_relay_frame(relay).await?;
            parse_file_transfer_json_response(message_type, &payload)
        }
    }
}

async fn upload_over_quic(
    app: &Window,
    job_id: &str,
    connection: &Connection,
    cancel: &CancellationToken,
    request: &FileTransferRequest,
    source_path: &Path,
    file_name: &str,
    total_bytes: u64,
) -> Result<FileTransferResponse, anyhow::Error> {
    let transfer_id = file_transfer_request_transfer_id(request)
        .ok_or_else(|| anyhow!("upload request missing transfer id"))?
        .to_string();
    let (mut send, mut recv) = connection.open_bi().await.map_err(|error| {
        let close_reason = connection
            .close_reason()
            .map(|reason| reason.to_string())
            .unwrap_or_else(|| "none".to_string());
        anyhow!("open file transfer quic upload stream: {error} (close reason: {close_reason})")
    })?;
    write_file_transfer_json_quic_frame(&mut send, request).await?;
    let ready_frame = tokio::select! {
        _ = cancel.cancelled() => return Ok(file_transfer_cancelled_response()),
        result = timeout(FILE_TRANSFER_RESPONSE_TIMEOUT, read_file_transfer_quic_frame(&mut recv)) => {
            result.map_err(|_| {
                anyhow!(
                    "upload ready response timed out after {} seconds",
                    FILE_TRANSFER_RESPONSE_TIMEOUT.as_secs()
                )
            })??
        }
    };
    let Some((ready_type, ready_payload)) = ready_frame else {
        return Err(anyhow!("upload ready response missing"));
    };
    let ready_response = parse_file_transfer_json_response(ready_type, &ready_payload)?;
    let resume_offset = match ready_response {
        FileTransferResponse::UploadReady {
            transfer_id: ready_transfer_id,
            resume_offset,
        } if ready_transfer_id == transfer_id => resume_offset,
        other => return Ok(other),
    };
    if resume_offset > total_bytes {
        return Err(anyhow!(
            "remote upload offset {resume_offset} exceeds local source length {total_bytes}"
        ));
    }
    emit_file_transfer_progress(
        app,
        job_id,
        "upload",
        file_name,
        resume_offset,
        total_bytes,
        Some("transferring"),
        Some("Uploading to remote host..."),
    );

    let mut file = File::open(source_path).context("open upload source")?;
    let mut buffer = vec![0u8; FILE_TRANSFER_DEFAULT_CHUNK_BYTES as usize];
    let mut bytes_done = resume_offset;
    if resume_offset > 0 {
        file.seek(SeekFrom::Start(resume_offset))
            .context("seek upload source to resume offset")?;
    }
    loop {
        if cancel.is_cancelled() {
            emit_file_transfer_progress(
                app,
                job_id,
                "upload",
                file_name,
                bytes_done,
                total_bytes,
                Some("finalizing"),
                Some("Cancelled"),
            );
            return Ok(file_transfer_cancelled_response());
        }
        let read = file.read(&mut buffer).context("read upload source")?;
        if read == 0 {
            break;
        }
        write_file_transfer_data_quic_frame(&mut send, &buffer[..read]).await?;
        bytes_done = bytes_done.saturating_add(read as u64);
        emit_file_transfer_progress(
            app,
            job_id,
            "upload",
            file_name,
            bytes_done,
            total_bytes,
            Some("transferring"),
            None,
        );
    }
    write_file_transfer_finish_quic_frame(&mut send).await?;
    let _ = send.finish();
    emit_file_transfer_progress(
        app,
        job_id,
        "upload",
        file_name,
        bytes_done,
        total_bytes,
        Some("finalizing"),
        Some("Waiting for remote finalize..."),
    );

    let final_frame = tokio::select! {
        _ = cancel.cancelled() => return Ok(file_transfer_cancelled_response()),
        result = timeout(FILE_TRANSFER_RESPONSE_TIMEOUT, read_file_transfer_quic_frame(&mut recv)) => {
            result.map_err(|_| {
                anyhow!(
                    "upload completion response timed out after {} seconds",
                    FILE_TRANSFER_RESPONSE_TIMEOUT.as_secs()
                )
            })??
        }
    };
    let Some((final_type, final_payload)) = final_frame else {
        return Err(anyhow!("upload completion response missing"));
    };
    parse_file_transfer_json_response(final_type, &final_payload)
}

async fn upload_over_relay(
    app: &Window,
    job_id: &str,
    relay: &mut RelayFileTransferConnection,
    cancel: &CancellationToken,
    request: &FileTransferRequest,
    source_path: &Path,
    file_name: &str,
    total_bytes: u64,
) -> Result<FileTransferResponse, anyhow::Error> {
    let transfer_id = file_transfer_request_transfer_id(request)
        .ok_or_else(|| anyhow!("upload request missing transfer id"))?
        .to_string();
    write_file_transfer_json_relay_frame(relay, request).await?;
    let (ready_type, ready_payload) = tokio::select! {
        _ = cancel.cancelled() => return Ok(file_transfer_cancelled_response()),
        result = timeout(FILE_TRANSFER_RESPONSE_TIMEOUT, read_file_transfer_relay_frame(relay)) => {
            result.map_err(|_| {
                anyhow!(
                    "upload relay ready response timed out after {} seconds",
                    FILE_TRANSFER_RESPONSE_TIMEOUT.as_secs()
                )
            })??
        }
    };
    let ready_response = parse_file_transfer_json_response(ready_type, &ready_payload)?;
    let resume_offset = match ready_response {
        FileTransferResponse::UploadReady {
            transfer_id: ready_transfer_id,
            resume_offset,
        } if ready_transfer_id == transfer_id => resume_offset,
        other => return Ok(other),
    };
    if resume_offset > total_bytes {
        return Err(anyhow!(
            "remote upload offset {resume_offset} exceeds local source length {total_bytes}"
        ));
    }
    emit_file_transfer_progress(
        app,
        job_id,
        "upload",
        file_name,
        resume_offset,
        total_bytes,
        Some("transferring"),
        Some("Uploading to remote host..."),
    );

    let mut file = File::open(source_path).context("open upload source")?;
    let mut buffer = vec![0u8; FILE_TRANSFER_DEFAULT_CHUNK_BYTES as usize];
    let mut bytes_done = resume_offset;
    if resume_offset > 0 {
        file.seek(SeekFrom::Start(resume_offset))
            .context("seek upload source to resume offset")?;
    }
    loop {
        if cancel.is_cancelled() {
            emit_file_transfer_progress(
                app,
                job_id,
                "upload",
                file_name,
                bytes_done,
                total_bytes,
                Some("finalizing"),
                Some("Cancelled"),
            );
            return Ok(file_transfer_cancelled_response());
        }
        let read = file.read(&mut buffer).context("read upload source")?;
        if read == 0 {
            break;
        }
        write_file_transfer_relay_frame(relay, FILE_TRANSFER_MSG_DATA, &buffer[..read]).await?;
        bytes_done = bytes_done.saturating_add(read as u64);
        emit_file_transfer_progress(
            app,
            job_id,
            "upload",
            file_name,
            bytes_done,
            total_bytes,
            Some("transferring"),
            None,
        );
    }
    write_file_transfer_relay_frame(relay, FILE_TRANSFER_MSG_FINISH, &[]).await?;
    emit_file_transfer_progress(
        app,
        job_id,
        "upload",
        file_name,
        bytes_done,
        total_bytes,
        Some("finalizing"),
        Some("Waiting for remote finalize..."),
    );

    let (final_type, final_payload) = tokio::select! {
        _ = cancel.cancelled() => return Ok(file_transfer_cancelled_response()),
        result = timeout(FILE_TRANSFER_RESPONSE_TIMEOUT, read_file_transfer_relay_frame(relay)) => {
            result.map_err(|_| {
                anyhow!(
                    "upload relay completion response timed out after {} seconds",
                    FILE_TRANSFER_RESPONSE_TIMEOUT.as_secs()
                )
            })??
        }
    };
    parse_file_transfer_json_response(final_type, &final_payload)
}

async fn download_over_quic(
    app: &Window,
    job_id: &str,
    connection: &Connection,
    cancel: &CancellationToken,
    request: &FileTransferRequest,
    local_destination: &Path,
    temp_path: &Path,
    conflict_mode: FileTransferConflictMode,
) -> Result<FileTransferResponse, anyhow::Error> {
    let transfer_id = file_transfer_request_transfer_id(request)
        .ok_or_else(|| anyhow!("download request missing transfer id"))?
        .to_string();
    let (mut send, mut recv) = connection.open_bi().await.map_err(|error| {
        let close_reason = connection
            .close_reason()
            .map(|reason| reason.to_string())
            .unwrap_or_else(|| "none".to_string());
        anyhow!("open file transfer quic download stream: {error} (close reason: {close_reason})")
    })?;
    write_file_transfer_json_quic_frame(&mut send, request).await?;
    let _ = send.finish();

    let (file_name, total_bytes, is_archive, resume_offset) = loop {
        if cancel.is_cancelled() {
            emit_file_transfer_progress(
                app,
                job_id,
                "download",
                "",
                0,
                0,
                Some("finalizing"),
                Some("Cancelled"),
            );
            return Ok(file_transfer_cancelled_response());
        }
        let Some((ready_type, ready_payload)) = read_file_transfer_quic_frame(&mut recv).await?
        else {
            return Err(anyhow!("download ready response missing"));
        };
        let ready_response = parse_file_transfer_json_response(ready_type, &ready_payload)?;
        match ready_response {
            FileTransferResponse::Progress {
                files_done: _,
                files_total: _,
                bytes_done,
                bytes_total,
                phase,
                message,
            } => {
                emit_file_transfer_progress(
                    app,
                    job_id,
                    "download",
                    "",
                    bytes_done,
                    bytes_total,
                    phase.as_deref(),
                    message.as_deref(),
                );
                continue;
            }
            FileTransferResponse::DownloadReady {
                transfer_id: ready_transfer_id,
                file_name,
                size_bytes,
                is_archive,
                resume_offset,
            } if ready_transfer_id == transfer_id => {
                break (file_name, size_bytes, is_archive, resume_offset)
            }
            other => return Ok(other),
        }
    };
    emit_file_transfer_progress(
        app,
        job_id,
        "download",
        &file_name,
        resume_offset,
        total_bytes,
        Some("transferring"),
        Some("Receiving from remote host..."),
    );

    let mut temp_cleanup = RemoveFileOnDrop::new(temp_path.to_path_buf());
    let local_resume_size = file_size_if_exists(temp_path)?;
    if resume_offset > local_resume_size {
        return Err(anyhow!(
            "remote download offset {resume_offset} exceeds local staged bytes {local_resume_size}"
        ));
    }
    let mut output = fs::OpenOptions::new()
        .create(true)
        .write(true)
        .open(temp_path)
        .context("open local download file")?;
    output
        .set_len(resume_offset)
        .context("truncate local download file to resume offset")?;
    output
        .seek(SeekFrom::Start(resume_offset))
        .context("seek local download file to resume offset")?;
    let mut bytes_done = resume_offset;
    loop {
        if cancel.is_cancelled() {
            drop(output); // close handle so deletion works on Windows
            emit_file_transfer_progress(
                app,
                job_id,
                "download",
                &file_name,
                bytes_done,
                total_bytes,
                Some("finalizing"),
                Some("Cancelled"),
            );
            return Ok(file_transfer_cancelled_response());
        }
        let Some((message_type, payload)) = read_file_transfer_quic_frame(&mut recv).await? else {
            return Err(anyhow!("download stream closed before finish"));
        };
        if message_type == FILE_TRANSFER_MSG_FINISH {
            break;
        }
        if message_type != FILE_TRANSFER_MSG_DATA {
            if message_type == FILE_TRANSFER_MSG_JSON {
                drop(output); // close handle so cleanup can remove the partial file
                let response = parse_file_transfer_json_response(message_type, &payload)?;
                return Ok(response);
            }
            return Err(anyhow!("unexpected download frame type"));
        }
        output
            .write_all(&payload)
            .context("write local download chunk")?;
        bytes_done = bytes_done.saturating_add(payload.len() as u64);
        emit_file_transfer_progress(
            app,
            job_id,
            "download",
            &file_name,
            bytes_done,
            total_bytes,
            Some("transferring"),
            None,
        );
    }
    output.flush().context("flush local download file")?;
    drop(output); // required on Windows before rename()/extract()
    emit_file_transfer_progress(
        app,
        job_id,
        "download",
        &file_name,
        bytes_done,
        total_bytes,
        Some("finalizing"),
        Some(if is_archive {
            "Extracting archive locally..."
        } else {
            "Finalizing downloaded file..."
        }),
    );

    let response = finalize_download_artifact(
        temp_path,
        local_destination,
        &file_name,
        is_archive,
        conflict_mode,
        &transfer_id,
        bytes_done,
    )
    .map_err(|error| anyhow!(error.to_string()))?;
    temp_cleanup.disarm();
    Ok(response)
}

async fn download_over_relay(
    app: &Window,
    job_id: &str,
    relay: &mut RelayFileTransferConnection,
    cancel: &CancellationToken,
    request: &FileTransferRequest,
    local_destination: &Path,
    temp_path: &Path,
    conflict_mode: FileTransferConflictMode,
) -> Result<FileTransferResponse, anyhow::Error> {
    write_file_transfer_json_relay_frame(relay, request).await?;
    let transfer_id = file_transfer_request_transfer_id(request)
        .ok_or_else(|| anyhow!("download request missing transfer id"))?
        .to_string();
    let (file_name, total_bytes, is_archive, resume_offset) = loop {
        if cancel.is_cancelled() {
            emit_file_transfer_progress(
                app,
                job_id,
                "download",
                "",
                0,
                0,
                Some("finalizing"),
                Some("Cancelled"),
            );
            return Ok(file_transfer_cancelled_response());
        }
        let (ready_type, ready_payload) = read_file_transfer_relay_frame(relay).await?;
        let ready_response = parse_file_transfer_json_response(ready_type, &ready_payload)?;
        match ready_response {
            FileTransferResponse::Progress {
                files_done: _,
                files_total: _,
                bytes_done,
                bytes_total,
                phase,
                message,
            } => {
                emit_file_transfer_progress(
                    app,
                    job_id,
                    "download",
                    "",
                    bytes_done,
                    bytes_total,
                    phase.as_deref(),
                    message.as_deref(),
                );
                continue;
            }
            FileTransferResponse::DownloadReady {
                transfer_id: ready_transfer_id,
                file_name,
                size_bytes,
                is_archive,
                resume_offset,
            } if ready_transfer_id == transfer_id => {
                break (file_name, size_bytes, is_archive, resume_offset)
            }
            other => return Ok(other),
        }
    };
    emit_file_transfer_progress(
        app,
        job_id,
        "download",
        &file_name,
        resume_offset,
        total_bytes,
        Some("transferring"),
        Some("Receiving from remote host..."),
    );

    let mut temp_cleanup = RemoveFileOnDrop::new(temp_path.to_path_buf());
    let local_resume_size = file_size_if_exists(temp_path)?;
    if resume_offset > local_resume_size {
        return Err(anyhow!(
            "remote download offset {resume_offset} exceeds local staged bytes {local_resume_size}"
        ));
    }
    let mut output = fs::OpenOptions::new()
        .create(true)
        .write(true)
        .open(temp_path)
        .context("open local download file")?;
    output
        .set_len(resume_offset)
        .context("truncate local download file to resume offset")?;
    output
        .seek(SeekFrom::Start(resume_offset))
        .context("seek local download file to resume offset")?;
    let mut bytes_done = resume_offset;
    loop {
        if cancel.is_cancelled() {
            drop(output); // close handle so deletion works on Windows
            emit_file_transfer_progress(
                app,
                job_id,
                "download",
                &file_name,
                bytes_done,
                total_bytes,
                Some("finalizing"),
                Some("Cancelled"),
            );
            return Ok(file_transfer_cancelled_response());
        }
        let (message_type, payload) = read_file_transfer_relay_frame(relay).await?;
        if message_type == FILE_TRANSFER_MSG_FINISH {
            break;
        }
        if message_type != FILE_TRANSFER_MSG_DATA {
            if message_type == FILE_TRANSFER_MSG_JSON {
                drop(output); // close handle so cleanup can remove the partial file
                let response = parse_file_transfer_json_response(message_type, &payload)?;
                return Ok(response);
            }
            return Err(anyhow!("unexpected download relay frame type"));
        }
        output
            .write_all(&payload)
            .context("write local download chunk")?;
        bytes_done = bytes_done.saturating_add(payload.len() as u64);
        emit_file_transfer_progress(
            app,
            job_id,
            "download",
            &file_name,
            bytes_done,
            total_bytes,
            Some("transferring"),
            None,
        );
    }
    output.flush().context("flush local download file")?;
    drop(output); // required on Windows before rename()/extract()
    emit_file_transfer_progress(
        app,
        job_id,
        "download",
        &file_name,
        bytes_done,
        total_bytes,
        Some("finalizing"),
        Some(if is_archive {
            "Extracting archive locally..."
        } else {
            "Finalizing downloaded file..."
        }),
    );

    let response = finalize_download_artifact(
        temp_path,
        local_destination,
        &file_name,
        is_archive,
        conflict_mode,
        &transfer_id,
        bytes_done,
    )
    .map_err(|error| anyhow!(error.to_string()))?;
    temp_cleanup.disarm();
    Ok(response)
}

fn emit_file_transfer_progress(
    app: &Window,
    job_id: &str,
    direction: &str,
    file_name: &str,
    bytes_done: u64,
    bytes_total: u64,
    phase: Option<&str>,
    message: Option<&str>,
) {
    let payload = FileTransferProgressPayload {
        job_id: job_id.to_string(),
        direction: direction.to_string(),
        file_name: file_name.to_string(),
        bytes_done,
        bytes_total,
        phase: phase.map(str::to_string),
        message: message.map(str::to_string),
    };
    emit_window(app, "file-transfer:progress", payload);
}

async fn write_file_transfer_json_quic_frame(
    send: &mut quinn::SendStream,
    request: &FileTransferRequest,
) -> Result<(), anyhow::Error> {
    let payload = serde_json::to_vec(request).context("serialize file transfer request")?;
    let frame = build_file_transfer_frame(FILE_TRANSFER_MSG_JSON, &payload)
        .context("build file transfer request frame")?;
    send.write_all(&frame)
        .await
        .context("write file transfer request frame")?;
    Ok(())
}

async fn write_file_transfer_data_quic_frame(
    send: &mut quinn::SendStream,
    payload: &[u8],
) -> Result<(), anyhow::Error> {
    let frame = build_file_transfer_frame(FILE_TRANSFER_MSG_DATA, payload)
        .context("build file transfer data frame")?;
    send.write_all(&frame)
        .await
        .context("write file transfer data frame")?;
    Ok(())
}

async fn write_file_transfer_finish_quic_frame(
    send: &mut quinn::SendStream,
) -> Result<(), anyhow::Error> {
    let frame = build_file_transfer_frame(FILE_TRANSFER_MSG_FINISH, &[])
        .context("build file transfer finish frame")?;
    send.write_all(&frame)
        .await
        .context("write file transfer finish frame")?;
    Ok(())
}

async fn read_file_transfer_quic_frame(
    recv: &mut quinn::RecvStream,
) -> Result<Option<(u8, Vec<u8>)>, anyhow::Error> {
    let mut header = [0u8; 5];
    if let Err(error) = recv.read_exact(&mut header).await {
        let message = error.to_string();
        if message.contains("finished early")
            || message.contains("closed")
            || message.contains("reset")
        {
            return Ok(None);
        }
        return Err(anyhow!("read file transfer quic header: {message}"));
    }

    let payload_len = u32::from_be_bytes([header[1], header[2], header[3], header[4]]) as usize;
    if payload_len > talos_protocol::FILE_TRANSFER_MAX_PAYLOAD_LEN {
        return Err(anyhow!("file transfer payload too large"));
    }
    let mut payload = vec![0u8; payload_len];
    if payload_len > 0 {
        recv.read_exact(&mut payload)
            .await
            .context("read file transfer quic payload")?;
    }
    Ok(Some((header[0], payload)))
}

async fn write_file_transfer_json_relay_frame(
    relay: &mut RelayFileTransferConnection,
    request: &FileTransferRequest,
) -> Result<(), anyhow::Error> {
    let payload = serde_json::to_vec(request).context("serialize file transfer request")?;
    write_file_transfer_relay_frame(relay, FILE_TRANSFER_MSG_JSON, &payload).await
}

async fn write_file_transfer_relay_frame(
    relay: &mut RelayFileTransferConnection,
    message_type: u8,
    payload: &[u8],
) -> Result<(), anyhow::Error> {
    let frame = build_file_transfer_frame(message_type, payload)
        .context("build file transfer relay frame")?;
    write_e2e_frame(
        &mut relay.writer,
        &relay.cipher,
        &mut relay.send_counter,
        &frame,
    )
    .await
}

async fn read_file_transfer_relay_frame(
    relay: &mut RelayFileTransferConnection,
) -> Result<(u8, Vec<u8>), anyhow::Error> {
    loop {
        let payload = read_e2e_frame_from(&mut relay.reader, &relay.cipher).await?;
        if payload == HEARTBEAT_PAYLOAD || payload == b"hello-world" {
            continue;
        }
        let frame =
            parse_file_transfer_frame(&payload).context("parse file transfer relay frame")?;
        return Ok((frame.message_type, frame.payload.to_vec()));
    }
}

fn parse_file_transfer_json_response(
    message_type: u8,
    payload: &[u8],
) -> Result<FileTransferResponse, anyhow::Error> {
    if message_type != FILE_TRANSFER_MSG_JSON {
        return Err(anyhow!(
            "unexpected file transfer frame type {message_type}"
        ));
    }
    serde_json::from_slice(payload).context("parse file transfer json response")
}

fn list_local_dir(path: &str) -> Result<FileTransferResponse, LocalTransferError> {
    let trimmed = path.trim();
    if trimmed.is_empty() || trimmed == "/" {
        return Ok(FileTransferResponse::ListDirResult {
            path: "/".to_string(),
            entries: list_local_roots(),
        });
    }

    let dir = normalize_existing_path(trimmed)?;
    if !dir.is_dir() {
        return Err(LocalTransferError::Message(
            "path is not a directory".to_string(),
        ));
    }
    let mut entries = fs::read_dir(&dir)?
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let entry_path = entry.path();
            let metadata = entry.metadata().ok()?;
            Some(FileTransferEntry {
                name: entry.file_name().to_string_lossy().to_string(),
                path: entry_path.to_string_lossy().to_string(),
                is_dir: metadata.is_dir(),
                size_bytes: if metadata.is_file() {
                    metadata.len()
                } else {
                    0
                },
                modified_unix_ms: metadata.modified().ok().and_then(system_time_to_unix_ms),
            })
        })
        .collect::<Vec<_>>();

    entries.sort_by(|a, b| {
        if a.is_dir != b.is_dir {
            return b.is_dir.cmp(&a.is_dir);
        }
        a.name.to_lowercase().cmp(&b.name.to_lowercase())
    });

    Ok(FileTransferResponse::ListDirResult {
        path: dir.to_string_lossy().to_string(),
        entries,
    })
}

fn rename_local_path(from_path: &str, to_path: &str) -> Result<(), LocalTransferError> {
    let from = normalize_existing_path(from_path.trim())?;
    let to_trimmed = to_path.trim();
    if to_trimmed.is_empty() {
        return Err(LocalTransferError::Message(
            "destination path must not be empty".to_string(),
        ));
    }
    let to = PathBuf::from(to_trimmed);
    if !to.is_absolute() {
        return Err(LocalTransferError::Message(
            "destination path must be absolute".to_string(),
        ));
    }
    if to.exists() {
        return Err(LocalTransferError::Message(
            "destination already exists".to_string(),
        ));
    }
    let parent = to.parent().ok_or_else(|| {
        LocalTransferError::Message("unable to resolve destination parent directory".to_string())
    })?;
    if !parent.exists() || !parent.is_dir() {
        return Err(LocalTransferError::Message(
            "destination parent is not a directory".to_string(),
        ));
    }
    fs::rename(&from, &to)?;
    Ok(())
}

fn delete_local_path(path: &str, recursive: bool) -> Result<(), LocalTransferError> {
    let target = normalize_existing_path(path.trim())?;
    if target.is_dir() {
        if recursive {
            fs::remove_dir_all(&target)?;
        } else {
            fs::remove_dir(&target)?;
        }
    } else {
        fs::remove_file(&target)?;
    }
    Ok(())
}

fn prepare_upload_bundle<F>(
    local_paths: &[String],
    cancelled: &AtomicBool,
    mut on_progress: F,
) -> Result<UploadBundle, LocalTransferError>
where
    F: FnMut(UploadPreparationProgress),
{
    if local_paths.is_empty() {
        return Err(LocalTransferError::Message(
            "no local paths selected".to_string(),
        ));
    }
    if cancelled.load(Ordering::Relaxed) {
        return Err(LocalTransferError::Message(
            FILE_TRANSFER_CANCELLED_MESSAGE.to_string(),
        ));
    }

    let paths = local_paths
        .iter()
        .map(|path| normalize_existing_path(path))
        .collect::<Result<Vec<_>, _>>()?;

    if paths.len() == 1 && paths[0].is_file() {
        let source_path = paths[0].clone();
        let file_name = source_path
            .file_name()
            .map(|value| value.to_string_lossy().to_string())
            .unwrap_or_else(|| "upload.bin".to_string());
        let size_bytes = fs::metadata(&source_path)?.len();
        on_progress(UploadPreparationProgress {
            files_done: 1,
            files_total: 1,
            bytes_done: size_bytes,
            bytes_total: size_bytes,
        });
        return Ok(UploadBundle {
            source_path,
            file_name,
            size_bytes,
            is_archive: false,
            extract_archive: false,
            cleanup_source: false,
        });
    }

    let (file_count, total_bytes, contains_dir) = summarize_local_paths(&paths)?;
    let should_zip = contains_dir
        || paths.len() > 1
        || file_count > FILE_TRANSFER_DEFAULT_ZIP_THRESHOLD_FILES as usize
        || total_bytes > FILE_TRANSFER_DEFAULT_ZIP_THRESHOLD_BYTES;
    if !should_zip {
        return Err(LocalTransferError::Message(
            "unable to prepare upload selection".to_string(),
        ));
    }

    let source_path = build_temp_transfer_path("talos_viewer_upload", "zip");
    on_progress(UploadPreparationProgress {
        files_done: 0,
        files_total: file_count,
        bytes_done: 0,
        bytes_total: total_bytes,
    });
    match create_zip_archive(
        &source_path,
        &paths,
        file_count,
        total_bytes,
        cancelled,
        &mut on_progress,
    ) {
        Ok(()) => {}
        Err(error) => {
            let _ = fs::remove_file(&source_path);
            return Err(error);
        }
    }
    let size_bytes = fs::metadata(&source_path)?.len();
    on_progress(UploadPreparationProgress {
        files_done: file_count,
        files_total: file_count,
        bytes_done: total_bytes,
        bytes_total: total_bytes,
    });
    let file_name = source_path
        .file_name()
        .map(|value| value.to_string_lossy().to_string())
        .unwrap_or_else(|| "upload.zip".to_string());
    Ok(UploadBundle {
        source_path,
        file_name,
        size_bytes,
        is_archive: true,
        extract_archive: true,
        cleanup_source: true,
    })
}

fn finalize_download_artifact(
    temp_path: &Path,
    destination: &Path,
    file_name: &str,
    is_archive: bool,
    conflict_mode: FileTransferConflictMode,
    transfer_id: &str,
    bytes_transferred: u64,
) -> Result<FileTransferResponse, LocalTransferError> {
    // Default behavior: if we return early (error/cancel/conflict), do not keep the temp file.
    // Callers that successfully rename/extract should disarm by moving/removing the file.
    let mut temp_cleanup = RemoveFileOnDrop::new(temp_path.to_path_buf());
    if is_archive {
        let extracted_entries = match extract_archive(temp_path, destination, conflict_mode) {
            Ok(entries) => entries,
            Err(LocalTransferError::Conflict { path, message }) => {
                return Ok(FileTransferResponse::Conflict { path, message });
            }
            Err(error) => return Err(error),
        };
        let _ = fs::remove_file(temp_path);
        temp_cleanup.disarm();
        return Ok(FileTransferResponse::TransferComplete {
            transfer_id: transfer_id.to_string(),
            bytes_transferred,
            extracted_entries,
        });
    }

    let mut target_path = destination.join(file_name);
    if target_path.exists() {
        match conflict_mode {
            FileTransferConflictMode::Prompt => {
                return Ok(FileTransferResponse::Conflict {
                    path: target_path.to_string_lossy().to_string(),
                    message: "destination already exists".to_string(),
                });
            }
            FileTransferConflictMode::Skip => {
                let _ = fs::remove_file(temp_path);
                temp_cleanup.disarm();
                return Ok(FileTransferResponse::TransferComplete {
                    transfer_id: transfer_id.to_string(),
                    bytes_transferred: 0,
                    extracted_entries: 0,
                });
            }
            FileTransferConflictMode::Overwrite => {
                remove_path_if_exists(&target_path)?;
            }
            FileTransferConflictMode::Rename => {
                target_path = next_available_path(&target_path);
            }
        }
    }

    fs::rename(temp_path, &target_path)?;
    temp_cleanup.disarm();
    Ok(FileTransferResponse::TransferComplete {
        transfer_id: transfer_id.to_string(),
        bytes_transferred,
        extracted_entries: 1,
    })
}

fn summarize_local_paths(paths: &[PathBuf]) -> Result<(usize, u64, bool), LocalTransferError> {
    let mut file_count = 0usize;
    let mut total_bytes = 0u64;
    let mut contains_dir = false;
    for path in paths {
        if path.is_dir() {
            contains_dir = true;
            for entry in WalkDir::new(path) {
                let entry = entry?;
                if entry.file_type().is_file() {
                    file_count = file_count.saturating_add(1);
                    total_bytes = total_bytes.saturating_add(entry.metadata()?.len());
                }
            }
        } else if path.is_file() {
            file_count = file_count.saturating_add(1);
            total_bytes = total_bytes.saturating_add(fs::metadata(path)?.len());
        }
    }
    Ok((file_count, total_bytes, contains_dir))
}

fn create_zip_archive<F>(
    archive_path: &Path,
    source_paths: &[PathBuf],
    file_count: usize,
    total_bytes: u64,
    cancelled: &AtomicBool,
    on_progress: &mut F,
) -> Result<(), LocalTransferError>
where
    F: FnMut(UploadPreparationProgress),
{
    let file = File::create(archive_path)?;
    let mut zip = ZipWriter::new(file);
    let compression = if should_use_store_mode(file_count, total_bytes) {
        CompressionMethod::Stored
    } else {
        CompressionMethod::Deflated
    };
    let file_options = SimpleFileOptions::default().compression_method(compression);
    let dir_options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    let mut files_done = 0usize;
    let mut bytes_done = 0u64;
    let mut last_emit = Instant::now();
    on_progress(UploadPreparationProgress {
        files_done,
        files_total: file_count,
        bytes_done,
        bytes_total: total_bytes,
    });

    for source_path in source_paths {
        if cancelled.load(Ordering::Relaxed) {
            return Err(LocalTransferError::Message(
                FILE_TRANSFER_CANCELLED_MESSAGE.to_string(),
            ));
        }
        let root_name = source_path
            .file_name()
            .map(|value| value.to_string_lossy().to_string())
            .unwrap_or_else(|| "item".to_string());
        if source_path.is_dir() {
            zip.add_directory(format!("{}/", to_zip_path(&root_name)), dir_options)?;
            for entry in WalkDir::new(source_path) {
                let entry = entry?;
                let entry_path = entry.path();
                let relative = entry_path
                    .strip_prefix(source_path)
                    .map_err(|error| LocalTransferError::Message(error.to_string()))?;
                if relative.as_os_str().is_empty() {
                    continue;
                }
                let mut archive_name = PathBuf::from(&root_name);
                archive_name.push(relative);
                let archive_name = to_zip_path(&archive_name.to_string_lossy());
                if entry.file_type().is_dir() {
                    zip.add_directory(format!("{archive_name}/"), dir_options)?;
                } else if entry.file_type().is_file() {
                    zip.start_file(archive_name, file_options)?;
                    let mut source = File::open(entry_path)?;
                    let mut buffer = [0u8; 262_144];
                    loop {
                        if cancelled.load(Ordering::Relaxed) {
                            return Err(LocalTransferError::Message(
                                FILE_TRANSFER_CANCELLED_MESSAGE.to_string(),
                            ));
                        }
                        let read = source.read(&mut buffer)?;
                        if read == 0 {
                            break;
                        }
                        zip.write_all(&buffer[..read])?;
                        bytes_done = bytes_done.saturating_add(read as u64).min(total_bytes);
                        if last_emit.elapsed() >= Duration::from_millis(90) {
                            on_progress(UploadPreparationProgress {
                                files_done,
                                files_total: file_count,
                                bytes_done,
                                bytes_total: total_bytes,
                            });
                            last_emit = Instant::now();
                        }
                    }
                    files_done = files_done.saturating_add(1);
                    on_progress(UploadPreparationProgress {
                        files_done,
                        files_total: file_count,
                        bytes_done,
                        bytes_total: total_bytes,
                    });
                }
            }
        } else {
            zip.start_file(to_zip_path(&root_name), file_options)?;
            let mut source = File::open(source_path)?;
            let mut buffer = [0u8; 262_144];
            loop {
                if cancelled.load(Ordering::Relaxed) {
                    return Err(LocalTransferError::Message(
                        FILE_TRANSFER_CANCELLED_MESSAGE.to_string(),
                    ));
                }
                let read = source.read(&mut buffer)?;
                if read == 0 {
                    break;
                }
                zip.write_all(&buffer[..read])?;
                bytes_done = bytes_done.saturating_add(read as u64).min(total_bytes);
                if last_emit.elapsed() >= Duration::from_millis(90) {
                    on_progress(UploadPreparationProgress {
                        files_done,
                        files_total: file_count,
                        bytes_done,
                        bytes_total: total_bytes,
                    });
                    last_emit = Instant::now();
                }
            }
            files_done = files_done.saturating_add(1);
            on_progress(UploadPreparationProgress {
                files_done,
                files_total: file_count,
                bytes_done,
                bytes_total: total_bytes,
            });
        }
    }

    zip.finish()?;
    on_progress(UploadPreparationProgress {
        files_done: file_count,
        files_total: file_count,
        bytes_done: total_bytes,
        bytes_total: total_bytes,
    });
    Ok(())
}

fn should_use_store_mode(file_count: usize, total_bytes: u64) -> bool {
    file_count >= FILE_TRANSFER_STORE_ARCHIVE_THRESHOLD_FILES as usize
        || total_bytes >= FILE_TRANSFER_STORE_ARCHIVE_THRESHOLD_BYTES
}

fn extract_archive(
    archive_path: &Path,
    destination: &Path,
    conflict_mode: FileTransferConflictMode,
) -> Result<u32, LocalTransferError> {
    let file = File::open(archive_path)?;
    let mut archive = ZipArchive::new(file)?;
    let mut extracted_entries = 0u32;

    for index in 0..archive.len() {
        let mut entry = archive.by_index(index)?;
        let Some(enclosed_name) = entry.enclosed_name().map(|value| value.to_owned()) else {
            continue;
        };
        let mut target_path = destination.join(enclosed_name);

        if entry.is_dir() {
            fs::create_dir_all(&target_path)?;
            continue;
        }
        if let Some(parent) = target_path.parent() {
            fs::create_dir_all(parent)?;
        }
        if target_path.exists() {
            match conflict_mode {
                FileTransferConflictMode::Prompt => {
                    return Err(LocalTransferError::Conflict {
                        path: target_path.to_string_lossy().to_string(),
                        message: "destination already exists".to_string(),
                    });
                }
                FileTransferConflictMode::Skip => {
                    continue;
                }
                FileTransferConflictMode::Overwrite => {
                    remove_path_if_exists(&target_path)?;
                }
                FileTransferConflictMode::Rename => {
                    target_path = next_available_path(&target_path);
                }
            }
        }

        let mut output = File::create(&target_path)?;
        std::io::copy(&mut entry, &mut output)?;
        extracted_entries = extracted_entries.saturating_add(1);
    }

    Ok(extracted_entries)
}

fn list_local_roots() -> Vec<FileTransferEntry> {
    #[cfg(windows)]
    {
        let mut roots = Vec::new();
        for drive in b'A'..=b'Z' {
            let path = format!("{}:\\", drive as char);
            let drive_path = PathBuf::from(&path);
            if drive_path.exists() {
                roots.push(FileTransferEntry {
                    name: path.clone(),
                    path,
                    is_dir: true,
                    size_bytes: 0,
                    modified_unix_ms: None,
                });
            }
        }
        roots
    }

    #[cfg(not(windows))]
    {
        let mut entries = fs::read_dir("/")
            .map(|read_dir| {
                read_dir
                    .filter_map(Result::ok)
                    .filter_map(|entry| {
                        let entry_path = entry.path();
                        let metadata = entry.metadata().ok()?;
                        if !metadata.is_dir() {
                            return None;
                        }
                        Some(FileTransferEntry {
                            name: entry.file_name().to_string_lossy().to_string(),
                            path: entry_path.to_string_lossy().to_string(),
                            is_dir: true,
                            size_bytes: 0,
                            modified_unix_ms: metadata
                                .modified()
                                .ok()
                                .and_then(system_time_to_unix_ms),
                        })
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        entries.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        entries
    }
}

fn normalize_existing_path(path: &str) -> Result<PathBuf, LocalTransferError> {
    let path = PathBuf::from(path);
    if !path.is_absolute() {
        return Err(LocalTransferError::Message(
            "path must be absolute".to_string(),
        ));
    }
    if !path.exists() {
        return Err(LocalTransferError::Message(
            "path does not exist".to_string(),
        ));
    }
    path.canonicalize()
        .map_err(|error| LocalTransferError::Message(error.to_string()))
}

fn normalize_existing_or_creatable_dir(path: &str) -> Result<PathBuf, LocalTransferError> {
    let path = PathBuf::from(path);
    if !path.is_absolute() {
        return Err(LocalTransferError::Message(
            "path must be absolute".to_string(),
        ));
    }
    if path.exists() {
        if !path.is_dir() {
            return Err(LocalTransferError::Message(
                "path is not a directory".to_string(),
            ));
        }
        return path
            .canonicalize()
            .map_err(|error| LocalTransferError::Message(error.to_string()));
    }

    fs::create_dir_all(&path)?;
    path.canonicalize()
        .map_err(|error| LocalTransferError::Message(error.to_string()))
}

fn remove_path_if_exists(path: &Path) -> Result<(), LocalTransferError> {
    if !path.exists() {
        return Ok(());
    }
    if path.is_dir() {
        fs::remove_dir_all(path)?;
    } else {
        fs::remove_file(path)?;
    }
    Ok(())
}

fn next_available_path(path: &Path) -> PathBuf {
    if !path.exists() {
        return path.to_path_buf();
    }

    let stem = path
        .file_stem()
        .map(|value| value.to_string_lossy().to_string())
        .unwrap_or_else(|| "file".to_string());
    let extension = path
        .extension()
        .map(|value| value.to_string_lossy().to_string())
        .unwrap_or_default();
    let parent = path.parent().map(Path::to_path_buf).unwrap_or_default();

    let mut index = 1usize;
    loop {
        let mut candidate_name = format!("{stem} ({index})");
        if !extension.is_empty() {
            candidate_name.push('.');
            candidate_name.push_str(&extension);
        }
        let candidate = parent.join(candidate_name);
        if !candidate.exists() {
            return candidate;
        }
        index = index.saturating_add(1);
    }
}

fn to_zip_path(path: &str) -> String {
    path.replace('\\', "/")
}

fn build_temp_transfer_path(prefix: &str, extension: &str) -> PathBuf {
    let millis = SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0);
    let mut path = std::env::temp_dir();
    path.push(format!("{prefix}_{millis}.{extension}"));
    path
}

fn system_time_to_unix_ms(value: SystemTime) -> Option<u64> {
    value
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_millis() as u64)
}

// ---------------------------------------------------------------------------
// Interactive shell transport (viewer ↔ agent, plain TCP)
// ---------------------------------------------------------------------------

/// State for an active shell TCP connection.
#[derive(Clone)]
struct ShellConnectionState(pub Arc<Mutex<Option<ShellConnection>>>);

impl Default for ShellConnectionState {
    fn default() -> Self {
        Self(Arc::new(Mutex::new(None)))
    }
}

struct ShellConnection {
    write_tx: mpsc::UnboundedSender<Vec<u8>>,
    transport: ShellTransportKind,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ShellTransportKind {
    Direct,
    Relay,
    Quic,
}

struct ShellTransportTask {
    write_tx: mpsc::UnboundedSender<Vec<u8>>,
    handle: tauri::async_runtime::JoinHandle<()>,
}

#[derive(Clone)]
struct ShellDirectConnectionState(pub Arc<Mutex<Option<ShellTransportTask>>>);

impl Default for ShellDirectConnectionState {
    fn default() -> Self {
        Self(Arc::new(Mutex::new(None)))
    }
}

#[derive(Clone)]
struct ShellRelayConnectionState(pub Arc<Mutex<Option<ShellTransportTask>>>);

impl Default for ShellRelayConnectionState {
    fn default() -> Self {
        Self(Arc::new(Mutex::new(None)))
    }
}

struct ShellQuicConnectionTask {
    write_tx: mpsc::UnboundedSender<Vec<u8>>,
    handle: tauri::async_runtime::JoinHandle<()>,
    shutdown: oneshot::Sender<()>,
    telemetry_cancel: CancellationToken,
}

#[derive(Clone)]
struct ShellQuicConnectionState(pub Arc<Mutex<Option<ShellQuicConnectionTask>>>);

impl Default for ShellQuicConnectionState {
    fn default() -> Self {
        Self(Arc::new(Mutex::new(None)))
    }
}

fn shell_transport_selected(state: &ShellConnectionState, transport: ShellTransportKind) -> bool {
    state
        .0
        .lock()
        .ok()
        .and_then(|guard| guard.as_ref().map(|conn| conn.transport == transport))
        .unwrap_or(false)
}

fn set_active_shell_transport(
    state: &ShellConnectionState,
    write_tx: mpsc::UnboundedSender<Vec<u8>>,
    transport: ShellTransportKind,
) {
    if let Ok(mut guard) = state.0.lock() {
        *guard = Some(ShellConnection {
            write_tx,
            transport,
        });
    }
}

fn clear_active_shell_transport(
    state: &ShellConnectionState,
    transport: Option<ShellTransportKind>,
) {
    if let Ok(mut guard) = state.0.lock() {
        let should_clear = match (transport, guard.as_ref()) {
            (Some(expected), Some(active)) => active.transport == expected,
            (Some(_), None) => false,
            (None, _) => true,
        };
        if should_clear {
            guard.take();
        }
    }
}

/// Connect to the agent's shell TCP port, authenticate, and start bridging I/O
/// via Tauri events.
#[tauri::command]
async fn shell_connect(
    app: Window,
    window_states: State<'_, AppWindowStates>,
    host: String,
    port: u16,
    token: String,
) -> Result<(), String> {
    use talos_protocol::{build_shell_frame, SHELL_MSG_AUTH};

    let window_state = window_states.get_or_create(app.label());
    let active_state = window_state.shell.clone();
    let direct_state = window_state.shell_direct.clone();
    let relay_state = window_state.shell_relay.clone();
    let quic_state = window_state.shell_quic.clone();

    if let Some(existing) = direct_state.0.lock().map_err(|e| e.to_string())?.take() {
        existing.handle.abort();
    }
    if let Some(existing) = relay_state.0.lock().map_err(|e| e.to_string())?.take() {
        existing.handle.abort();
    }
    if let Some(existing) = quic_state.0.lock().map_err(|e| e.to_string())?.take() {
        let _ = existing.shutdown.send(());
        existing.handle.abort();
    }
    clear_active_shell_transport(&active_state, None);

    // Connect TCP.
    let addr = format!("{host}:{port}");
    let connect_timeout = Duration::from_secs(
        std::env::var("RMM_SHELL_CONNECT_TIMEOUT_SECS")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(10),
    );
    let stream = timeout(connect_timeout, TcpStream::connect(&addr))
        .await
        .map_err(|_| format!("TCP connect to {addr} timed out"))?
        .map_err(|e| format!("TCP connect to {addr}: {e}"))?;
    let (mut tcp_read, mut tcp_write) = stream.into_split();

    // Send auth frame.
    let auth_frame = build_shell_frame(SHELL_MSG_AUTH, token.as_bytes())
        .map_err(|e| format!("build auth frame: {e}"))?;
    tcp_write
        .write_all(&auth_frame)
        .await
        .map_err(|e| format!("send auth: {e}"))?;
    tcp_write
        .flush()
        .await
        .map_err(|e| format!("flush auth: {e}"))?;

    // Channel for outgoing frames (input, resize).
    let (write_tx, mut write_rx) = mpsc::unbounded_channel::<Vec<u8>>();

    let app_clone = app.clone();
    let active_for_task = active_state.clone();
    let direct_state_for_task = direct_state.0.clone();

    // Spawn task that handles both reading from agent and writing to agent.
    let handle = tauri::async_runtime::spawn(async move {
        // Writer sub-task: channel → TCP.
        let writer_task = {
            async {
                while let Some(frame) = write_rx.recv().await {
                    if tcp_write.write_all(&frame).await.is_err() {
                        break;
                    }
                    if tcp_write.flush().await.is_err() {
                        break;
                    }
                }
            }
        };

        // Reader sub-task: TCP → Tauri events.
        let reader_task = {
            let app = app_clone.clone();
            async move {
                use talos_protocol::{
                    parse_shell_exit_payload, SHELL_MSG_ERROR, SHELL_MSG_EXIT, SHELL_MSG_OUTPUT,
                };
                loop {
                    // Read frame header: 1B type + 2B length.
                    let mut header = [0u8; 3];
                    if tcp_read.read_exact(&mut header).await.is_err() {
                        clear_active_shell_transport(
                            &active_for_task,
                            Some(ShellTransportKind::Direct),
                        );
                        emit_window(&app, "shell:error", "connection closed");
                        break;
                    }
                    let msg_type = header[0];
                    let length = u16::from_be_bytes([header[1], header[2]]) as usize;

                    // Read payload.
                    let mut payload = vec![0u8; length];
                    if length > 0 && tcp_read.read_exact(&mut payload).await.is_err() {
                        clear_active_shell_transport(
                            &active_for_task,
                            Some(ShellTransportKind::Direct),
                        );
                        emit_window(&app, "shell:error", "connection lost");
                        break;
                    }

                    match msg_type {
                        SHELL_MSG_OUTPUT => {
                            // Emit raw bytes as a Vec<u8> — frontend will decode.
                            emit_window(&app, "shell:data", payload);
                        }
                        SHELL_MSG_EXIT => {
                            let code = parse_shell_exit_payload(&payload).unwrap_or(0);
                            clear_active_shell_transport(
                                &active_for_task,
                                Some(ShellTransportKind::Direct),
                            );
                            emit_window(&app, "shell:exit", code);
                            break;
                        }
                        SHELL_MSG_ERROR => {
                            let msg = String::from_utf8_lossy(&payload).to_string();
                            clear_active_shell_transport(
                                &active_for_task,
                                Some(ShellTransportKind::Direct),
                            );
                            emit_window(&app, "shell:error", msg);
                            break;
                        }
                        _ => {
                            debug!("shell: ignoring unknown frame type 0x{:02x}", msg_type);
                        }
                    }
                }
            }
        };

        // Run reader and writer concurrently; when either finishes, drop the other.
        tokio::select! {
            _ = writer_task => {}
            _ = reader_task => {}
        }
        if let Ok(mut guard) = direct_state_for_task.lock() {
            guard.take();
        }
    });

    direct_state
        .0
        .lock()
        .map_err(|e| e.to_string())?
        .replace(ShellTransportTask {
            write_tx: write_tx.clone(),
            handle,
        });
    set_active_shell_transport(&active_state, write_tx, ShellTransportKind::Direct);

    Ok(())
}

/// Connect shell over relay (internet path, outbound TCP 443 from both ends).
#[tauri::command]
async fn shell_connect_relay(
    app: Window,
    window_states: State<'_, AppWindowStates>,
    session_id: String,
    relay_url: String,
    e2e_key: String,
    _token: String,
) -> Result<String, String> {
    use talos_protocol::{
        parse_shell_exit_payload, SHELL_MSG_ERROR, SHELL_MSG_EXIT, SHELL_MSG_OUTPUT,
    };

    let window_state = window_states.get_or_create(app.label());
    let active_state = window_state.shell.clone();
    let relay_state = window_state.shell_relay.clone();
    let telemetry_state = window_state.shell_connection_telemetry.clone();
    if let Some(existing) = relay_state.0.lock().map_err(|e| e.to_string())?.take() {
        existing.handle.abort();
    }

    let relay_target = parse_relay_target(&relay_url).map_err(|e| e.to_string())?;
    let addr = format!("{}:{}", relay_target.host, relay_target.port);
    let connect_timeout = relay_connect_timeout();
    let tcp_stream = timeout(connect_timeout, TcpStream::connect(addr.clone()))
        .await
        .map_err(|_| "connect relay tcp timed out".to_string())?
        .map_err(|e| format!("connect relay tcp: {e}"))?;
    tcp_stream
        .set_nodelay(true)
        .map_err(|e| format!("set relay TCP_NODELAY: {e}"))?;

    let tls_config = build_tls_config().map_err(|e| e.to_string())?;
    let connector = TlsConnector::from(Arc::new(tls_config));
    let server_name =
        ServerName::try_from(relay_target.host.clone()).map_err(|e| format!("server name: {e}"))?;
    let mut stream = timeout(connect_timeout, connector.connect(server_name, tcp_stream))
        .await
        .map_err(|_| "relay tls connect timed out".to_string())?
        .map_err(|e| format!("relay tls connect: {e}"))?;

    let request = format!(
        "GET /relay/{session_id} HTTP/1.1\r\nHost: {host}\r\n\r\n",
        session_id = session_id,
        host = relay_target.host
    );
    stream
        .write_all(request.as_bytes())
        .await
        .map_err(|e| format!("write relay request: {e}"))?;
    timeout(connect_timeout, read_http_response(&mut stream))
        .await
        .map_err(|_| "read relay response timed out".to_string())?
        .map_err(|e| format!("read relay response: {e}"))?;

    let key_bytes = BASE64_URL_SAFE_NO_PAD
        .decode(e2e_key.trim())
        .or_else(|_| BASE64_STANDARD.decode(e2e_key.trim()))
        .map_err(|e| format!("decode relay e2e key: {e}"))?;
    let cipher = build_e2e_cipher(&key_bytes).map_err(|e| e.to_string())?;

    let mut send_counter = 0u64;
    write_e2e_frame(&mut stream, &cipher, &mut send_counter, b"hello-world")
        .await
        .map_err(|e| format!("send relay hello frame: {e}"))?;

    let (mut relay_reader, relay_writer) = tokio::io::split(stream);
    start_connection_telemetry(
        app.clone(),
        telemetry_state.clone(),
        ControlState::default(),
        ConnectionStatePayload {
            session_kind: "system_shell".to_string(),
            transport: "relay".to_string(),
            connection_type: "relay".to_string(),
            encryption_label: "TLS + E2E ChaCha20-Poly1305".to_string(),
            encryption_details: Some(
                "Shell relay traffic is protected by TLS to the relay and encrypted end-to-end with ChaCha20-Poly1305."
                    .to_string(),
            ),
            remote_addr: Some(addr.clone()),
            viewer_reflex: None,
            agent_reflex: None,
            agent_local_addrs: Vec::new(),
            connect_ms: None,
            relay_tcp_ms: None,
            relay_tls_ms: None,
            relay_handshake_ms: None,
            capture_type: None,
        },
    );
    let writer_cipher = build_e2e_cipher(&key_bytes).map_err(|e| e.to_string())?;
    let (write_tx, write_rx) = mpsc::unbounded_channel::<Vec<u8>>();

    let app_clone = app.clone();
    let active_for_task = active_state.clone();
    let telemetry_for_task = telemetry_state.clone();
    let relay_state_for_task = relay_state.0.clone();
    let handle = tauri::async_runtime::spawn(async move {
        let writer_handle = tokio::spawn(run_relay_writer(
            relay_writer,
            writer_cipher,
            Duration::from_secs(15),
            write_rx,
        ));

        loop {
            let payload = match read_e2e_frame_from(&mut relay_reader, &cipher).await {
                Ok(payload) => payload,
                Err(err) => {
                    telemetry_for_task.clear_transport("relay");
                    if shell_transport_selected(&active_for_task, ShellTransportKind::Relay) {
                        clear_active_shell_transport(
                            &active_for_task,
                            Some(ShellTransportKind::Relay),
                        );
                        emit_window(&app_clone, "shell:error", format!("relay closed: {err}"));
                    }
                    emit_window(
                        &app_clone,
                        "shell:relay:error",
                        format!("relay closed: {err}"),
                    );
                    break;
                }
            };
            if payload == HEARTBEAT_PAYLOAD || payload == b"hello-world" {
                continue;
            }
            let (message_type, frame_payload) = match parse_shell_wire_frame(&payload) {
                Ok(value) => value,
                Err(err) => {
                    warn!(error = %err, "invalid shell relay frame");
                    continue;
                }
            };
            match message_type {
                SHELL_MSG_OUTPUT => {
                    if shell_transport_selected(&active_for_task, ShellTransportKind::Relay) {
                        emit_window(&app_clone, "shell:data", frame_payload.to_vec());
                    }
                }
                SHELL_MSG_EXIT => {
                    telemetry_for_task.clear_transport("relay");
                    let code = parse_shell_exit_payload(frame_payload).unwrap_or(0);
                    if shell_transport_selected(&active_for_task, ShellTransportKind::Relay) {
                        clear_active_shell_transport(
                            &active_for_task,
                            Some(ShellTransportKind::Relay),
                        );
                        emit_window(&app_clone, "shell:exit", code);
                    }
                    emit_window(&app_clone, "shell:relay:ended", ());
                    break;
                }
                SHELL_MSG_ERROR => {
                    telemetry_for_task.clear_transport("relay");
                    let msg = String::from_utf8_lossy(frame_payload).to_string();
                    if shell_transport_selected(&active_for_task, ShellTransportKind::Relay) {
                        clear_active_shell_transport(
                            &active_for_task,
                            Some(ShellTransportKind::Relay),
                        );
                        emit_window(&app_clone, "shell:error", msg.clone());
                    }
                    emit_window(&app_clone, "shell:relay:error", msg);
                    break;
                }
                _ => {}
            }
        }
        telemetry_for_task.clear_transport("relay");
        writer_handle.abort();
        if let Ok(mut guard) = relay_state_for_task.lock() {
            guard.take();
        }
    });

    relay_state
        .0
        .lock()
        .map_err(|e| e.to_string())?
        .replace(ShellTransportTask { write_tx, handle });
    Ok(format!("relay connected ({session_id})"))
}

/// Send terminal input to the agent shell.
///
/// Accepts a `String` (UTF-8) rather than raw bytes to avoid serialisation
/// issues between the JS frontend and serde.  xterm.js `onData` already
/// provides a string, and ConPTY stdin expects UTF-8.
#[tauri::command]
async fn shell_write(
    window: Window,
    window_states: State<'_, AppWindowStates>,
    data: String,
) -> Result<(), String> {
    use talos_protocol::{build_shell_frame, SHELL_MSG_INPUT};

    let state = window_states.get_or_create(window.label()).shell;

    let frame = build_shell_frame(SHELL_MSG_INPUT, data.as_bytes())
        .map_err(|e| format!("build input frame: {e}"))?;

    let guard = state.0.lock().map_err(|e| format!("lock: {e}"))?;
    if let Some(conn) = guard.as_ref() {
        conn.write_tx
            .send(frame)
            .map_err(|_| "shell connection closed".to_string())?;
    } else {
        return Err("no shell connection".to_string());
    }
    Ok(())
}

/// Resize the agent's shell terminal.
#[tauri::command]
async fn shell_resize(
    window: Window,
    window_states: State<'_, AppWindowStates>,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    use talos_protocol::{build_shell_frame, build_shell_resize_payload, SHELL_MSG_RESIZE};

    let state = window_states.get_or_create(window.label()).shell;

    let resize = build_shell_resize_payload(cols, rows);
    let frame = build_shell_frame(SHELL_MSG_RESIZE, &resize)
        .map_err(|e| format!("build resize frame: {e}"))?;

    let guard = state.0.lock().map_err(|e| format!("lock: {e}"))?;
    if let Some(conn) = guard.as_ref() {
        conn.write_tx
            .send(frame)
            .map_err(|_| "shell connection closed".to_string())?;
    } else {
        return Err("no shell connection".to_string());
    }
    Ok(())
}

/// Disconnect the shell session.
#[tauri::command]
async fn shell_disconnect(
    window: Window,
    window_states: State<'_, AppWindowStates>,
) -> Result<(), String> {
    let window_state = window_states.get_or_create(window.label());
    clear_active_shell_transport(&window_state.shell, None);
    window_state
        .shell_connection_telemetry
        .clear_transport("relay");
    window_state
        .shell_connection_telemetry
        .clear_transport("quic");
    if let Some(task) = window_state
        .shell_direct
        .0
        .lock()
        .map_err(|e| e.to_string())?
        .take()
    {
        task.handle.abort();
    }
    if let Some(task) = window_state
        .shell_relay
        .0
        .lock()
        .map_err(|e| e.to_string())?
        .take()
    {
        task.handle.abort();
    }
    if let Some(task) = window_state
        .shell_quic
        .0
        .lock()
        .map_err(|e| e.to_string())?
        .take()
    {
        let _ = task.shutdown.send(());
        task.handle.abort();
    }
    Ok(())
}

#[tauri::command]
async fn shell_select_relay(
    window: Window,
    window_states: State<'_, AppWindowStates>,
    token: String,
) -> Result<(), String> {
    use talos_protocol::{build_shell_frame, SHELL_MSG_AUTH};

    let window_state = window_states.get_or_create(window.label());
    let write_tx = window_state
        .shell_relay
        .0
        .lock()
        .map_err(|e| e.to_string())?
        .as_ref()
        .map(|task| task.write_tx.clone())
        .ok_or_else(|| "no shell relay connection".to_string())?;
    let auth_frame = build_shell_frame(SHELL_MSG_AUTH, token.as_bytes())
        .map_err(|e| format!("build shell auth frame: {e}"))?;
    write_tx
        .send(auth_frame)
        .map_err(|_| "shell relay connection closed".to_string())?;
    set_active_shell_transport(&window_state.shell, write_tx, ShellTransportKind::Relay);
    Ok(())
}

#[tauri::command]
async fn shell_select_quic(
    window: Window,
    window_states: State<'_, AppWindowStates>,
    token: String,
) -> Result<(), String> {
    use talos_protocol::{build_shell_frame, SHELL_MSG_AUTH};

    let window_state = window_states.get_or_create(window.label());
    let write_tx = window_state
        .shell_quic
        .0
        .lock()
        .map_err(|e| e.to_string())?
        .as_ref()
        .map(|task| task.write_tx.clone())
        .ok_or_else(|| "no shell quic connection".to_string())?;
    let auth_frame = build_shell_frame(SHELL_MSG_AUTH, token.as_bytes())
        .map_err(|e| format!("build shell auth frame: {e}"))?;
    write_tx
        .send(auth_frame)
        .map_err(|_| "shell quic connection closed".to_string())?;
    set_active_shell_transport(&window_state.shell, write_tx, ShellTransportKind::Quic);
    Ok(())
}

#[tauri::command]
async fn shell_disconnect_relay(
    window: Window,
    window_states: State<'_, AppWindowStates>,
) -> Result<(), String> {
    let window_state = window_states.get_or_create(window.label());
    clear_active_shell_transport(&window_state.shell, Some(ShellTransportKind::Relay));
    window_state
        .shell_connection_telemetry
        .clear_transport("relay");
    if let Some(task) = window_state
        .shell_relay
        .0
        .lock()
        .map_err(|e| e.to_string())?
        .take()
    {
        task.handle.abort();
    }
    emit_window(&window, "shell:relay:ended", ());
    Ok(())
}

#[tauri::command]
async fn shell_disconnect_quic(
    window: Window,
    window_states: State<'_, AppWindowStates>,
) -> Result<(), String> {
    let window_state = window_states.get_or_create(window.label());
    clear_active_shell_transport(&window_state.shell, Some(ShellTransportKind::Quic));
    window_state
        .shell_connection_telemetry
        .clear_transport("quic");
    if let Some(task) = window_state
        .shell_quic
        .0
        .lock()
        .map_err(|e| e.to_string())?
        .take()
    {
        let _ = task.shutdown.send(());
        task.telemetry_cancel.cancel();
        task.handle.abort();
    }
    emit_window(&window, "shell:quic:ended", ());
    Ok(())
}

/// Connect shell over QUIC, racing LAN vs reflex just like remote desktop.
#[tauri::command]
async fn shell_connect_quic(
    app: Window,
    window_states: State<'_, AppWindowStates>,
    session_id: String,
    token: String,
    agent_reflex: ReflexAddress,
    agent_host: Option<String>,
    agent_local_addrs: Option<Vec<LocalAddr>>,
    psk_cert_pem: String,
    api_base: String,
    quic_timeout_ms: Option<u64>,
) -> Result<String, String> {
    use talos_protocol::{
        parse_shell_exit_payload, SHELL_MSG_ERROR, SHELL_MSG_EXIT, SHELL_MSG_OUTPUT,
    };

    let window_state = window_states.get_or_create(app.label());
    let active_state = window_state.shell.clone();
    let quic_state = window_state.shell_quic.clone();
    let telemetry_state = window_state.shell_connection_telemetry.clone();

    if let Some(existing) = quic_state.0.lock().map_err(|e| e.to_string())?.take() {
        let _ = existing.shutdown.send(());
        existing.handle.abort();
    }

    let viewer_addrs = viewer_local_addrs();
    let lan_candidate = match &agent_local_addrs {
        Some(addrs) => pick_lan_candidate(&viewer_addrs, addrs),
        None => agent_host.filter(|h| !h.trim().is_empty()),
    };
    let reflex_addr: SocketAddr = format!("{}:{}", agent_reflex.ip, agent_reflex.port)
        .parse()
        .map_err(|e: std::net::AddrParseError| e.to_string())?;
    let lan_addr = lan_candidate
        .as_ref()
        .map(|ip| format!("{}:{}", ip, agent_reflex.port).parse::<SocketAddr>())
        .transpose()
        .map_err(|e: std::net::AddrParseError| e.to_string())?;

    info!(
        session_id = %session_id,
        lan = ?lan_candidate,
        reflex = %reflex_addr,
        "shell_connect_quic invoked"
    );

    let socket = UdpSocket::bind("0.0.0.0:0").map_err(|e| e.to_string())?;
    socket.set_nonblocking(true).map_err(|e| e.to_string())?;

    let viewer_reflex = tokio::task::spawn_blocking({
        let stun_socket = socket.try_clone().ok();
        move || -> Result<SocketAddr, anyhow::Error> {
            let stun_socket = stun_socket.ok_or_else(|| anyhow!("stun socket clone failed"))?;
            query_configured_stun_reflex(stun_socket)
        }
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string())?;

    let reflex_url = format!(
        "{}/api/rmm/shell/session/{}/viewer-reflex?token={}",
        api_base.trim_end_matches('/'),
        session_id,
        urlencoding::encode(&token)
    );
    let reflex_body = serde_json::json!({
        "ip": viewer_reflex.ip().to_string(),
        "port": viewer_reflex.port(),
    });
    let _ = Client::new()
        .post(reflex_url)
        .json(&reflex_body)
        .send()
        .await;

    let mut endpoint = Endpoint::new(
        EndpointConfig::default(),
        None,
        socket,
        Arc::new(TokioRuntime),
    )
    .map_err(|e| e.to_string())?;
    let client_config = build_client_config(&psk_cert_pem).map_err(|e| e.to_string())?;
    endpoint.set_default_client_config(client_config);

    let quic_timeout = Duration::from_millis(quic_timeout_ms.unwrap_or(2000));
    let (connection, connection_type, connect_ms) = if let Some(lan_addr) = lan_addr {
        let lan_started_at = Instant::now();
        let mut lan_handle = tokio::spawn(run_quic_with_timeout(
            endpoint.clone(),
            session_id.clone(),
            lan_addr,
            quic_timeout,
        ));
        let reflex_started_at = Instant::now();
        let mut reflex_handle = tokio::spawn(run_quic_with_timeout(
            endpoint.clone(),
            session_id.clone(),
            reflex_addr,
            quic_timeout,
        ));

        let mut errors: Vec<String> = Vec::new();
        let mut lan_done = false;
        let mut reflex_done = false;
        loop {
            tokio::select! {
                result = &mut lan_handle, if !lan_done => {
                    match result {
                        Ok(Ok(conn)) => {
                            reflex_handle.abort();
                            info!(session_id = %session_id, source = "lan", "shell quic connected");
                            break (
                                conn,
                                "lan_direct".to_string(),
                                lan_started_at.elapsed().as_millis().min(u128::from(u64::MAX)) as u64,
                            );
                        }
                        Ok(Err(e)) => {
                            errors.push(format!("lan: {e}"));
                            lan_done = true;
                        }
                        Err(e) => {
                            errors.push(format!("lan task: {e}"));
                            lan_done = true;
                        }
                    }
                }
                result = &mut reflex_handle, if !reflex_done => {
                    match result {
                        Ok(Ok(conn)) => {
                            lan_handle.abort();
                            info!(session_id = %session_id, source = "reflex", "shell quic connected");
                            break (
                                conn,
                                "hole_punch".to_string(),
                                reflex_started_at.elapsed().as_millis().min(u128::from(u64::MAX)) as u64,
                            );
                        }
                        Ok(Err(e)) => {
                            errors.push(format!("reflex: {e}"));
                            reflex_done = true;
                        }
                        Err(e) => {
                            errors.push(format!("reflex task: {e}"));
                            reflex_done = true;
                        }
                    }
                }
                else => {
                    return Err(format!("shell quic connect failed: {}", errors.join("; ")));
                }
            }
            if lan_done && reflex_done {
                return Err(format!("shell quic connect failed: {}", errors.join("; ")));
            }
        }
    } else {
        let reflex_started_at = Instant::now();
        (
            run_quic_with_timeout(
                endpoint.clone(),
                session_id.clone(),
                reflex_addr,
                quic_timeout,
            )
            .await
            .map_err(|e| format!("shell quic connect failed: {e}"))?,
            "hole_punch".to_string(),
            reflex_started_at
                .elapsed()
                .as_millis()
                .min(u128::from(u64::MAX)) as u64,
        )
    };

    start_connection_telemetry(
        app.clone(),
        telemetry_state.clone(),
        ControlState::default(),
        ConnectionStatePayload {
            session_kind: "system_shell".to_string(),
            transport: "quic".to_string(),
            connection_type,
            encryption_label: "Pinned QUIC TLS".to_string(),
            encryption_details: Some(
                "Shell QUIC session authenticated with the per-session pinned certificate."
                    .to_string(),
            ),
            remote_addr: Some(connection.remote_address().to_string()),
            viewer_reflex: Some(ReflexAddress {
                ip: viewer_reflex.ip().to_string(),
                port: viewer_reflex.port(),
            }),
            agent_reflex: Some(agent_reflex.clone()),
            agent_local_addrs: agent_local_addrs.clone().unwrap_or_default(),
            connect_ms: Some(connect_ms),
            relay_tcp_ms: None,
            relay_tls_ms: None,
            relay_handshake_ms: None,
            capture_type: None,
        },
    );

    let (mut send, mut recv) = connection
        .open_bi()
        .await
        .map_err(|e| format!("open shell quic bi-stream: {e}"))?;

    let (write_tx, mut write_rx) = mpsc::unbounded_channel::<Vec<u8>>();
    let app_clone = app.clone();
    let active_for_task = active_state.clone();
    let telemetry_for_task = telemetry_state.clone();
    let quic_state_for_task = quic_state.0.clone();
    let telemetry_cancel = CancellationToken::new();
    let telemetry_cancel_for_task = telemetry_cancel.clone();
    let (shutdown_tx, mut shutdown_rx) = oneshot::channel();
    let handle = tauri::async_runtime::spawn(async move {
        let writer_task = async {
            while let Some(frame) = write_rx.recv().await {
                if send.write_all(&frame).await.is_err() {
                    break;
                }
                if send.flush().await.is_err() {
                    break;
                }
            }
            let _ = send.finish();
        };

        let reader_task = {
            let app = app_clone.clone();
            let telemetry_for_reader = telemetry_for_task.clone();
            async move {
                loop {
                    let mut header = [0u8; 3];
                    if recv.read_exact(&mut header).await.is_err() {
                        telemetry_for_reader.clear_transport("quic");
                        if shell_transport_selected(&active_for_task, ShellTransportKind::Quic) {
                            clear_active_shell_transport(
                                &active_for_task,
                                Some(ShellTransportKind::Quic),
                            );
                            emit_window(&app, "shell:error", "quic connection closed");
                        }
                        emit_window(&app, "shell:quic:error", "quic connection closed");
                        break;
                    }
                    let msg_type = header[0];
                    let length = u16::from_be_bytes([header[1], header[2]]) as usize;
                    let mut payload = vec![0u8; length];
                    if length > 0 && recv.read_exact(&mut payload).await.is_err() {
                        telemetry_for_reader.clear_transport("quic");
                        if shell_transport_selected(&active_for_task, ShellTransportKind::Quic) {
                            clear_active_shell_transport(
                                &active_for_task,
                                Some(ShellTransportKind::Quic),
                            );
                            emit_window(&app, "shell:error", "quic connection lost");
                        }
                        emit_window(&app, "shell:quic:error", "quic connection lost");
                        break;
                    }
                    match msg_type {
                        SHELL_MSG_OUTPUT => {
                            if shell_transport_selected(&active_for_task, ShellTransportKind::Quic)
                            {
                                emit_window(&app, "shell:data", payload);
                            }
                        }
                        SHELL_MSG_EXIT => {
                            telemetry_for_reader.clear_transport("quic");
                            let code = parse_shell_exit_payload(&payload).unwrap_or(0);
                            if shell_transport_selected(&active_for_task, ShellTransportKind::Quic)
                            {
                                clear_active_shell_transport(
                                    &active_for_task,
                                    Some(ShellTransportKind::Quic),
                                );
                                emit_window(&app, "shell:exit", code);
                            }
                            emit_window(&app, "shell:quic:ended", ());
                            break;
                        }
                        SHELL_MSG_ERROR => {
                            telemetry_for_reader.clear_transport("quic");
                            let msg = String::from_utf8_lossy(&payload).to_string();
                            if shell_transport_selected(&active_for_task, ShellTransportKind::Quic)
                            {
                                clear_active_shell_transport(
                                    &active_for_task,
                                    Some(ShellTransportKind::Quic),
                                );
                                emit_window(&app, "shell:error", msg.clone());
                            }
                            emit_window(&app, "shell:quic:error", msg);
                            break;
                        }
                        _ => {}
                    }
                }
            }
        };

        tokio::select! {
            _ = &mut shutdown_rx => {
                telemetry_for_task.clear_transport("quic");
                emit_window(&app_clone, "shell:quic:ended", ());
            }
            _ = writer_task => {}
            _ = reader_task => {}
        }
        telemetry_cancel_for_task.cancel();
        telemetry_for_task.clear_transport("quic");
        if let Ok(mut guard) = quic_state_for_task.lock() {
            guard.take();
        }
    });

    quic_state
        .0
        .lock()
        .map_err(|e| e.to_string())?
        .replace(ShellQuicConnectionTask {
            write_tx,
            handle,
            shutdown: shutdown_tx,
            telemetry_cancel,
        });
    Ok(format!("quic connected ({session_id})"))
}

fn parse_shell_wire_frame(frame: &[u8]) -> Result<(u8, &[u8]), anyhow::Error> {
    if frame.len() < 3 {
        return Err(anyhow!("shell frame too short"));
    }
    let message_type = frame[0];
    let payload_len = u16::from_be_bytes([frame[1], frame[2]]) as usize;
    if frame.len() != 3 + payload_len {
        return Err(anyhow!("shell frame length mismatch"));
    }
    Ok((message_type, &frame[3..]))
}

// ── Theme persistence via Windows Registry (HKCU\Software\Talos\Viewer) ──

#[cfg(windows)]
mod theme_registry {
    use windows_sys::Win32::Foundation::ERROR_SUCCESS;
    use windows_sys::Win32::System::Registry::{
        RegCloseKey, RegCreateKeyExW, RegQueryValueExW, RegSetValueExW, HKEY, HKEY_CURRENT_USER,
        KEY_READ, KEY_WRITE, REG_OPTION_NON_VOLATILE, REG_SZ,
    };

    const SUBKEY: &str = "Software\\Talos\\Viewer";
    const VALUE_NAME: &str = "Theme";

    fn to_wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn wide_subkey() -> Vec<u16> {
        to_wide(SUBKEY)
    }
    fn wide_value_name() -> Vec<u16> {
        to_wide(VALUE_NAME)
    }

    pub fn read_theme() -> Option<String> {
        unsafe {
            let mut hkey: HKEY = std::ptr::null_mut();
            let subkey = wide_subkey();
            let rc = RegCreateKeyExW(
                HKEY_CURRENT_USER,
                subkey.as_ptr(),
                0,
                std::ptr::null(),
                REG_OPTION_NON_VOLATILE,
                KEY_READ,
                std::ptr::null(),
                &mut hkey,
                std::ptr::null_mut(),
            );
            if rc != ERROR_SUCCESS {
                return None;
            }

            let value_name = wide_value_name();
            let mut buf = [0u8; 128];
            let mut buf_len = buf.len() as u32;
            let mut reg_type = 0u32;
            let rc = RegQueryValueExW(
                hkey,
                value_name.as_ptr(),
                std::ptr::null(),
                &mut reg_type,
                buf.as_mut_ptr(),
                &mut buf_len,
            );
            RegCloseKey(hkey);

            if rc != ERROR_SUCCESS || reg_type != REG_SZ || buf_len < 2 {
                return None;
            }
            let slice = &buf[..buf_len as usize];
            let wide: Vec<u16> = slice
                .chunks_exact(2)
                .map(|c| u16::from_le_bytes([c[0], c[1]]))
                .collect();
            let s = String::from_utf16_lossy(&wide);
            let trimmed = s.trim_end_matches('\0').to_string();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed)
            }
        }
    }

    pub fn write_theme(theme: &str) -> bool {
        unsafe {
            let mut hkey: HKEY = std::ptr::null_mut();
            let subkey = wide_subkey();
            let rc = RegCreateKeyExW(
                HKEY_CURRENT_USER,
                subkey.as_ptr(),
                0,
                std::ptr::null(),
                REG_OPTION_NON_VOLATILE,
                KEY_WRITE,
                std::ptr::null(),
                &mut hkey,
                std::ptr::null_mut(),
            );
            if rc != ERROR_SUCCESS {
                return false;
            }

            let value_name = wide_value_name();
            let wide_data = to_wide(theme);
            let byte_len = wide_data.len() * 2;
            let rc = RegSetValueExW(
                hkey,
                value_name.as_ptr(),
                0,
                REG_SZ,
                wide_data.as_ptr() as *const u8,
                byte_len as u32,
            );
            RegCloseKey(hkey);
            rc == ERROR_SUCCESS
        }
    }
}

#[cfg(windows)]
mod startup_registry {
    use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
    use windows_sys::Win32::System::Registry::{
        RegCloseKey, RegCreateKeyExW, RegDeleteValueW, RegQueryValueExW, RegSetValueExW, HKEY,
        HKEY_CURRENT_USER, KEY_READ, KEY_WRITE, REG_OPTION_NON_VOLATILE, REG_SZ,
    };

    const SETTINGS_SUBKEY: &str = "Software\\Talos\\Viewer";
    const SETTINGS_VALUE_NAME: &str = "StartOnLogin";
    const RUN_SUBKEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
    const RUN_VALUE_NAME: &str = "TalosViewer";

    fn to_wide(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn read_string_value(hkey: HKEY, subkey: &str, value_name: &str) -> Option<String> {
        unsafe {
            let mut key: HKEY = std::ptr::null_mut();
            let subkey = to_wide(subkey);
            let rc = RegCreateKeyExW(
                hkey,
                subkey.as_ptr(),
                0,
                std::ptr::null(),
                REG_OPTION_NON_VOLATILE,
                KEY_READ,
                std::ptr::null(),
                &mut key,
                std::ptr::null_mut(),
            );
            if rc != ERROR_SUCCESS {
                return None;
            }

            let value_name = to_wide(value_name);
            let mut buf = vec![0u8; 512];
            let mut buf_len = buf.len() as u32;
            let mut reg_type = 0u32;
            let rc = RegQueryValueExW(
                key,
                value_name.as_ptr(),
                std::ptr::null(),
                &mut reg_type,
                buf.as_mut_ptr(),
                &mut buf_len,
            );
            RegCloseKey(key);

            if rc != ERROR_SUCCESS || reg_type != REG_SZ || buf_len < 2 {
                return None;
            }
            let slice = &buf[..buf_len as usize];
            let wide: Vec<u16> = slice
                .chunks_exact(2)
                .map(|c| u16::from_le_bytes([c[0], c[1]]))
                .collect();
            let value = String::from_utf16_lossy(&wide)
                .trim_end_matches('\0')
                .trim()
                .to_string();
            if value.is_empty() {
                None
            } else {
                Some(value)
            }
        }
    }

    fn write_string_value(hkey: HKEY, subkey: &str, value_name: &str, value: &str) -> bool {
        unsafe {
            let mut key: HKEY = std::ptr::null_mut();
            let subkey = to_wide(subkey);
            let rc = RegCreateKeyExW(
                hkey,
                subkey.as_ptr(),
                0,
                std::ptr::null(),
                REG_OPTION_NON_VOLATILE,
                KEY_WRITE,
                std::ptr::null(),
                &mut key,
                std::ptr::null_mut(),
            );
            if rc != ERROR_SUCCESS {
                return false;
            }

            let value_name = to_wide(value_name);
            let wide_value = to_wide(value);
            let rc = RegSetValueExW(
                key,
                value_name.as_ptr(),
                0,
                REG_SZ,
                wide_value.as_ptr() as *const u8,
                (wide_value.len() * 2) as u32,
            );
            RegCloseKey(key);
            rc == ERROR_SUCCESS
        }
    }

    fn delete_value(hkey: HKEY, subkey: &str, value_name: &str) -> bool {
        unsafe {
            let mut key: HKEY = std::ptr::null_mut();
            let subkey = to_wide(subkey);
            let rc = RegCreateKeyExW(
                hkey,
                subkey.as_ptr(),
                0,
                std::ptr::null(),
                REG_OPTION_NON_VOLATILE,
                KEY_WRITE,
                std::ptr::null(),
                &mut key,
                std::ptr::null_mut(),
            );
            if rc != ERROR_SUCCESS {
                return false;
            }

            let value_name = to_wide(value_name);
            let rc = RegDeleteValueW(key, value_name.as_ptr());
            RegCloseKey(key);
            rc == ERROR_SUCCESS || rc == ERROR_FILE_NOT_FOUND
        }
    }

    fn startup_command() -> Result<String, String> {
        let exe = std::env::current_exe().map_err(|err| err.to_string())?;
        Ok(format!(
            "\"{}\" {}",
            exe.display(),
            super::VIEWER_START_ON_LOGIN_ARG
        ))
    }

    fn apply_auto_start(enabled: bool) -> Result<(), String> {
        if enabled {
            let command = startup_command()?;
            if !write_string_value(HKEY_CURRENT_USER, RUN_SUBKEY, RUN_VALUE_NAME, &command) {
                return Err("failed to write Windows Run entry".to_string());
            }
        } else if !delete_value(HKEY_CURRENT_USER, RUN_SUBKEY, RUN_VALUE_NAME) {
            return Err("failed to remove Windows Run entry".to_string());
        }
        Ok(())
    }

    pub fn auto_start_enabled() -> bool {
        let raw = read_string_value(HKEY_CURRENT_USER, SETTINGS_SUBKEY, SETTINGS_VALUE_NAME);
        raw.map(|value| {
            let normalized = value.trim().to_ascii_lowercase();
            !matches!(normalized.as_str(), "0" | "false" | "off" | "no")
        })
        .unwrap_or(true)
    }

    pub fn sync_auto_start() -> Result<bool, String> {
        let enabled = auto_start_enabled();
        apply_auto_start(enabled)?;
        Ok(enabled)
    }

    pub fn set_auto_start_enabled(enabled: bool) -> Result<(), String> {
        let persisted = if enabled { "1" } else { "0" };
        if !write_string_value(
            HKEY_CURRENT_USER,
            SETTINGS_SUBKEY,
            SETTINGS_VALUE_NAME,
            persisted,
        ) {
            return Err("failed to persist startup preference".to_string());
        }
        apply_auto_start(enabled)
    }

    pub fn launched_via_autostart() -> bool {
        std::env::args().any(|arg| arg.eq_ignore_ascii_case(super::VIEWER_START_ON_LOGIN_ARG))
    }
}

#[cfg(target_os = "macos")]
mod startup_registry {
    use super::VIEWER_START_ON_LOGIN_ARG;
    use std::{
        env, fs,
        path::{Path, PathBuf},
    };

    const LAUNCH_AGENT_LABEL: &str = "com.talos.viewer";
    const LAUNCH_AGENT_FILE_NAME: &str = "com.talos.viewer.plist";

    fn launch_agent_path() -> Result<PathBuf, String> {
        let home = env::var("HOME").map_err(|_| "HOME is not set".to_string())?;
        Ok(Path::new(&home)
            .join("Library")
            .join("LaunchAgents")
            .join(LAUNCH_AGENT_FILE_NAME))
    }

    fn plist_contents() -> Result<String, String> {
        let exe = env::current_exe().map_err(|err| err.to_string())?;
        let exe = xml_escape(&exe.display().to_string());
        Ok(format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>{label}</string>
  <key>ProgramArguments</key>
  <array>
    <string>{exe}</string>
    <string>{arg}</string>
  </array>
  <key>RunAtLoad</key>
  <true/>
</dict>
</plist>
"#,
            label = LAUNCH_AGENT_LABEL,
            exe = exe,
            arg = VIEWER_START_ON_LOGIN_ARG
        ))
    }

    pub fn auto_start_enabled() -> bool {
        launch_agent_path()
            .map(|path| path.exists())
            .unwrap_or(false)
    }

    pub fn sync_auto_start() -> Result<bool, String> {
        Ok(auto_start_enabled())
    }

    pub fn set_auto_start_enabled(enabled: bool) -> Result<(), String> {
        let path = launch_agent_path()?;
        if enabled {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).map_err(|err| err.to_string())?;
            }
            fs::write(&path, plist_contents()?).map_err(|err| err.to_string())?;
            return Ok(());
        }
        match fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(err.to_string()),
        }
    }

    pub fn launched_via_autostart() -> bool {
        std::env::args().any(|arg| arg == VIEWER_START_ON_LOGIN_ARG)
    }

    fn xml_escape(input: &str) -> String {
        input
            .replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('"', "&quot;")
            .replace('\'', "&apos;")
    }
}

#[cfg(all(not(windows), not(target_os = "macos")))]
mod startup_registry {
    pub fn auto_start_enabled() -> bool {
        false
    }

    pub fn sync_auto_start() -> Result<bool, String> {
        Ok(false)
    }

    pub fn set_auto_start_enabled(_enabled: bool) -> Result<(), String> {
        Ok(())
    }

    pub fn launched_via_autostart() -> bool {
        false
    }
}

#[tauri::command]
fn get_theme_preference() -> String {
    #[cfg(windows)]
    {
        theme_registry::read_theme().unwrap_or_else(|| "dark".to_string())
    }
    #[cfg(not(windows))]
    {
        "dark".to_string()
    }
}

#[tauri::command]
fn set_theme_preference(theme: String) -> Result<(), String> {
    let normalized = match theme.to_lowercase().as_str() {
        "light" => "light",
        _ => "dark",
    };
    #[cfg(windows)]
    {
        if theme_registry::write_theme(normalized) {
            Ok(())
        } else {
            Err("Failed to write theme preference to registry".to_string())
        }
    }
    #[cfg(not(windows))]
    {
        let _ = normalized;
        Ok(())
    }
}

#[tauri::command]
fn remember_update_api_base(api_base: String) -> Result<(), String> {
    updater::remember_update_api_base(&api_base).map_err(|error| error.to_string())
}

#[tauri::command]
async fn viewer_check_for_updates(
    app: tauri::AppHandle,
    state: State<'_, ViewerUpdateState>,
) -> Result<updater::ManualUpdateCheckResult, String> {
    let Some(manager) = state.manager.clone() else {
        return Err("viewer updater unavailable".to_string());
    };
    manager
        .manual_check(&app)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn viewer_apply_staged_update(
    app: tauri::AppHandle,
    state: State<'_, ViewerUpdateState>,
) -> Result<bool, String> {
    let Some(manager) = state.manager.clone() else {
        return Err("viewer updater unavailable".to_string());
    };
    manager
        .apply_staged_update(&app)
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn viewer_complete_update_exit_cleanup() {
    updater::complete_update_exit_cleanup();
}

#[cfg(test)]
mod tests {
    #[test]
    fn viewport_occlusions_preserve_the_input_contract_on_every_platform() {
        let valid = r#"{"x":-10,"y":20,"width":30,"height":40}"#;
        assert!(serde_json::from_str::<super::ViewportOcclusionRect>(valid).is_ok());
        let invalid = r#"{"x":0,"y":0,"width":-1,"height":40}"#;
        assert!(serde_json::from_str::<super::ViewportOcclusionRect>(invalid).is_err());
    }

    use super::*;
    use std::sync::mpsc::channel;
    use std::time::Duration;
    use tauri::{test::mock_app, Listener, WebviewWindowBuilder};

    #[test]
    fn emit_window_targets_only_the_owning_session_window() {
        let app = mock_app();
        let session_one = WebviewWindowBuilder::new(&app, "session-one", Default::default())
            .build()
            .expect("build session-one window");
        let session_two = WebviewWindowBuilder::new(&app, "session-two", Default::default())
            .build()
            .expect("build session-two window");
        let (tx, rx) = channel();

        let tx_one = tx.clone();
        session_one.listen("shell:data", move |event| {
            tx_one
                .send(("session-one", event.payload().to_string()))
                .expect("record session-one event");
        });

        let tx_two = tx.clone();
        session_two.listen("shell:data", move |event| {
            tx_two
                .send(("session-two", event.payload().to_string()))
                .expect("record session-two event");
        });

        emit_to_label(
            &app,
            session_one.label(),
            "shell:data",
            "first-session-output",
        );

        let received = rx
            .recv_timeout(Duration::from_secs(1))
            .expect("target window received event");
        assert_eq!(received.0, "session-one");
        assert_eq!(
            serde_json::from_str::<String>(&received.1).expect("event payload is string"),
            "first-session-output"
        );
        assert!(
            rx.recv_timeout(Duration::from_millis(100)).is_err(),
            "event leaked to another session window"
        );
    }

    #[test]
    fn bmp_frame_encoding_uses_top_down_bgra_pixels() {
        let encoded = encode_argb_bmp_base64(1, 1, &[0x1122_3344]).expect("encode bmp frame");
        let bytes = BASE64_STANDARD.decode(encoded).expect("decode bmp frame");

        assert_eq!(&bytes[0..2], b"BM");
        assert_eq!(i32::from_le_bytes(bytes[22..26].try_into().unwrap()), -1);
        assert_eq!(&bytes[54..58], &[0x44, 0x33, 0x22, 0x11]);
    }

    #[test]
    fn legacy_vp8_payload_prefix_accepts_bounded_payload() {
        assert_eq!(
            parse_legacy_vp8_payload_prefix(1024u32.to_le_bytes()),
            LegacyVp8PayloadPrefix::Payload(1024)
        );
    }

    #[test]
    fn legacy_vp8_payload_prefix_rejects_midstream_dkif() {
        assert_eq!(
            parse_legacy_vp8_payload_prefix(*b"DKIF"),
            LegacyVp8PayloadPrefix::MidstreamIvfHeader
        );
    }

    #[test]
    fn legacy_vp8_payload_prefix_rejects_oversized_payload() {
        let payload_len = (MAX_LEGACY_VP8_PAYLOAD_LEN as u32).saturating_add(1);

        assert_eq!(
            parse_legacy_vp8_payload_prefix(payload_len.to_le_bytes()),
            LegacyVp8PayloadPrefix::TooLarge(payload_len as usize)
        );
    }

    #[test]
    fn capture_output_switch_control_event_deserializes() {
        let event =
            serde_json::from_str::<ControlEvent>(r#"{"type":"captureOutputSwitch","index":1}"#)
                .expect("deserialize capture output switch event");

        match event {
            ControlEvent::CaptureOutputSwitch { index } => assert_eq!(index, 1),
            other => panic!("unexpected control event: {other:?}"),
        }
    }

    #[test]
    fn capture_output_switch_control_event_builds_wire_frame() {
        let (frame, is_mouse_move) = build_control_message(
            ControlEvent::CaptureOutputSwitch { index: 2 },
            Some((1920, 1080)),
        )
        .expect("build capture output switch frame");

        assert!(!is_mouse_move);
        let parsed =
            talos_protocol::parse_control_frame(&frame).expect("parse capture output switch frame");
        assert_eq!(parsed.message_type, CONTROL_TYPE_CAPTURE_OUTPUT_SWITCH);
        assert_eq!(parsed.payload, &2u32.to_be_bytes());
    }

    #[test]
    fn capture_outputs_event_payload_maps_stream_metadata() {
        let metadata = serde_json::from_str::<RemoteDesktopStreamMetaPayload>(
            r#"{
                "captureType": "macos_screencapturekit_h264",
                "activeIndex": 1,
                "captureOutputs": [
                    {
                        "index": 0,
                        "displayId": 11,
                        "name": "Main Display",
                        "width": 1920,
                        "height": 1080,
                        "primary": true
                    },
                    {
                        "index": 1,
                        "displayId": 22,
                        "name": "Display 2 (1280x720)",
                        "width": 1280,
                        "height": 720,
                        "originX": 1920.0,
                        "pointWidth": 1280.0,
                        "primary": false
                    }
                ]
            }"#,
        )
        .expect("parse remote desktop stream metadata");

        let event = capture_outputs_event_payload(&metadata).expect("capture outputs event");
        assert_eq!(event.active_index, 1);
        assert_eq!(
            event.capture_type.as_deref(),
            Some("macos_screencapturekit_h264")
        );
        assert_eq!(event.outputs.len(), 2);
        assert_eq!(event.outputs[1].index, 1);
        assert_eq!(event.outputs[1].display_id, Some(22));
        assert_eq!(event.outputs[1].width, Some(1280));
        assert_eq!(event.outputs[1].height, Some(720));
        assert_eq!(event.outputs[1].primary, Some(false));
    }

    #[cfg(not(windows))]
    #[test]
    fn nonwindows_remote_desktop_frame_populates_snapshot_cache() {
        let label = "session-cache-test";
        let frame = DecodedFrame {
            width: 2,
            height: 1,
            fps: 30,
            argb: vec![0xFF11_2233, 0xFF44_5566],
        };

        cache_nonwindows_remote_desktop_frame_for_label(label, &frame);

        let cached = NON_WINDOWS_REMOTE_DESKTOP_FRAMES
            .get()
            .and_then(|cache| {
                cache
                    .lock()
                    .ok()
                    .and_then(|guard| guard.get(label).cloned())
            })
            .expect("frame cached for snapshot capture");
        assert_eq!(cached.width, frame.width);
        assert_eq!(cached.height, frame.height);
        assert_eq!(cached.argb, frame.argb);
    }
}

fn main() {
    load_viewer_dotenv();

    // On Windows we run with GUI subsystem (no console). Opt-in to a debug console
    // with `RMM_DEBUG=debug` or `RMM_DEBUG=true`.
    init_debug_console();

    rustls::crypto::ring::default_provider()
        .install_default()
        .expect("install rustls CryptoProvider once per process");

    if let Err(err) = init_file_logging() {
        eprintln!("failed to initialize viewer file logging: {err}");
    } else {
        info!("talos_viewer startup");
    }
    if startup_registry::launched_via_autostart() && !startup_registry::auto_start_enabled() {
        info!("viewer autostart launch ignored because startup preference is disabled");
        return;
    }
    if let Err(err) = updater::promote_pending_updater() {
        warn!(error = %err, "failed to promote pending viewer updater");
    }
    match updater::take_update_notice() {
        Ok(Some(version)) => {
            show_info_dialog(
                "Talos Viewer Update",
                &format!("Talos Viewer was updated to version {version}."),
            );
        }
        Ok(None) => {}
        Err(err) => {
            warn!(error = %err, "failed to read viewer update completion notice");
        }
    }

    #[cfg(windows)]
    if std::env::var_os("RMM_VIEWER_DISABLE_INPUT_CAPTURE")
        .is_some_and(|v| v == "1" || v == "true" || v == "TRUE")
    {
        info!("RMM_VIEWER_DISABLE_INPUT_CAPTURE: native viewport input capture disabled (no keyboard/mouse forwarded to remote)");
    }

    // Windows key suppression: RIDEV_NOHOTKEYS (C++ FFI) is the sole
    // mechanism.  Registration is handled in viewport_wndproc WM_SETFOCUS /
    // WM_KILLFOCUS.  win_key_block_init is a no-op stub kept for compat.
    #[cfg(windows)]
    {
        let ret = unsafe { win_key_block_init() };
        if ret != 0 {
            warn!("win_key_block_init failed ({ret})");
        }
    }

    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_deep_link::init())
        .plugin(tauri_plugin_single_instance::init(|app, args, _cwd| {
            // When the app is already running, Windows launches a new process for `rmm://...`.
            // The single-instance plugin forwards argv here so we can handle it in-process.
            let url = args.iter().find(|arg| arg.starts_with("rmm://")).cloned();

            if let Some(url) = url {
                let pending_urls = app.state::<PendingSessionUrls>().inner().clone();
                if let Err(err) = queue_session_window(app, &pending_urls, url) {
                    warn!(error = %err, "failed to open single-instance session window");
                }
            } else {
                // No deep link args: stay background-resident (tray only).
            }
        }))
        .manage(AppWindowStates::default())
        .manage(PendingSessionUrls::default())
        .setup(|app| {
            let handle = app.handle();
            let auto_start_enabled = match startup_registry::sync_auto_start() {
                Ok(enabled) => enabled,
                Err(err) => {
                    warn!(error = %err, "failed to synchronize viewer startup entry");
                    startup_registry::auto_start_enabled()
                }
            };
            let update_manager = match updater::UpdateManager::from_env() {
                Ok(manager) => {
                    manager.start_background_task(handle.clone());
                    Some(manager)
                }
                Err(err) => {
                    warn!(error = %err, "failed to initialize viewer updater");
                    None
                }
            };
            app.manage(ViewerUpdateState {
                manager: update_manager,
            });

            if let Some(window) = handle.get_webview_window(MAIN_WINDOW_LABEL) {
                apply_session_window_size(&window);
                disable_browser_accelerator_keys(&window);
            }

            // Keep a background-resident viewer: tray icon provides show/hide + quit.
            // Note: hide-to-tray logic is gated on the tray actually existing.
            if handle.tray_by_id(TRAY_ID).is_none() {
                let Some(icon) = handle.default_window_icon().cloned() else {
                    warn!("default window icon unavailable; tray icon disabled");
                    return Ok(());
                };

                let check_updates_item = tauri::menu::MenuItem::with_id(
                    handle,
                    TRAY_MENU_CHECK_UPDATES_ID,
                    "Check for Updates",
                    true,
                    None::<&str>,
                )?;
                let about_item = tauri::menu::MenuItem::with_id(
                    handle,
                    TRAY_MENU_ABOUT_ID,
                    "About",
                    true,
                    None::<&str>,
                )?;
                let auto_start_item = tauri::menu::CheckMenuItem::with_id(
                    handle,
                    TRAY_MENU_AUTOSTART_ID,
                    "Start on Login",
                    true,
                    auto_start_enabled,
                    None::<&str>,
                )?;
                let separator_primary = tauri::menu::PredefinedMenuItem::separator(handle)?;
                let separator_secondary = tauri::menu::PredefinedMenuItem::separator(handle)?;
                let exit_item = tauri::menu::MenuItem::with_id(
                    handle,
                    TRAY_MENU_EXIT_ID,
                    "Exit",
                    true,
                    None::<&str>,
                )?;
                let menu = tauri::menu::Menu::with_items(
                    handle,
                    &[
                        &check_updates_item,
                        &about_item,
                        &separator_primary,
                        &auto_start_item,
                        &separator_secondary,
                        &exit_item,
                    ],
                )?;
                let check_updates_item_for_cb = check_updates_item.clone();
                let auto_start_item_for_cb = auto_start_item.clone();

                tauri::tray::TrayIconBuilder::with_id(TRAY_ID)
                    .icon(icon)
                    .tooltip("Talos Viewer")
                    .menu(&menu)
                    // Right-click shows the context menu.
                    .show_menu_on_left_click(false)
                    .on_menu_event(move |app, event| match event.id().as_ref() {
                        TRAY_MENU_EXIT_ID => app.exit(0),
                        TRAY_MENU_ABOUT_ID => {
                            show_about_dialog(app);
                        }
                        TRAY_MENU_CHECK_UPDATES_ID => {
                            start_tray_update_check(app.clone(), check_updates_item_for_cb.clone());
                        }
                        TRAY_MENU_AUTOSTART_ID => {
                            let enabled = auto_start_item_for_cb
                                .is_checked()
                                .unwrap_or(auto_start_enabled);
                            if let Err(err) = startup_registry::set_auto_start_enabled(enabled) {
                                let _ = auto_start_item_for_cb.set_checked(!enabled);
                                show_error_dialog("Startup Preference", &err);
                            } else {
                                info!(enabled, "viewer startup preference updated");
                            }
                        }
                        _ => {}
                    })
                    .build(handle)?;
            }

            // Start background-only (tray resident). The dispatcher window can be shown
            // via tray `Show`, but should not appear on cold launch.
            if let Some(window) = handle.get_webview_window(MAIN_WINDOW_LABEL) {
                let _ = window.hide();
            }
            set_macos_activation_policy_accessory();
            if let Some(url) = launch_arg_url() {
                let pending_urls = handle.state::<PendingSessionUrls>().inner().clone();
                if let Err(err) = open_initial_session_window(handle, &pending_urls, url) {
                    warn!(error = %err, "failed to open initial session window");
                }
            }
            Ok(())
        });

    builder
        .on_window_event(|window, event| {
            #[cfg(windows)]
            {
                let window_states = window.app_handle().state::<AppWindowStates>();
                let state = window_states.get_or_create(window.label());
                move_child_on_window_event(&state.viewport.inner, window, event);
            }
            if let WindowEvent::CloseRequested { api, .. } = event {
                // Dispatcher window is hidden to tray; session windows (added later)
                // will close normally.
                if window.label() == MAIN_WINDOW_LABEL
                    && window.app_handle().tray_by_id(TRAY_ID).is_some()
                {
                    api.prevent_close();
                    let _ = window.hide();
                    return;
                }

                #[cfg(windows)]
                force_release_forwarded_win_key("main.rs:on_window_event:CloseRequested");
                let _ = window
                    .app_handle()
                    .state::<PendingSessionUrls>()
                    .take(window.label());
                let window_states = window.app_handle().state::<AppWindowStates>();
                let state = window_states.remove(window.label()).unwrap_or_default();

                if let Some(task) = state.quic.0.lock().ok().and_then(|mut guard| guard.take()) {
                    let _ = task.shutdown.send(());
                    task.handle.abort();
                }
                if let Some(task) = state
                    .registry_quic
                    .0
                    .lock()
                    .ok()
                    .and_then(|mut guard| guard.take())
                {
                    let _ = task.shutdown.send(());
                    task.handle.abort();
                }
                if let Some(handle) = state.relay.0.lock().ok().and_then(|mut guard| guard.take()) {
                    handle.abort();
                }
                if let Some(handle) = state
                    .registry_relay
                    .0
                    .lock()
                    .ok()
                    .and_then(|mut guard| guard.take())
                {
                    handle.abort();
                }
                if let Ok(mut guard) = state.shell.0.lock() {
                    guard.take();
                }
                if let Some(task) = state
                    .shell_direct
                    .0
                    .lock()
                    .ok()
                    .and_then(|mut guard| guard.take())
                {
                    task.handle.abort();
                }
                if let Some(task) = state
                    .shell_relay
                    .0
                    .lock()
                    .ok()
                    .and_then(|mut guard| guard.take())
                {
                    task.handle.abort();
                }
                if let Some(task) = state
                    .shell_quic
                    .0
                    .lock()
                    .ok()
                    .and_then(|mut guard| guard.take())
                {
                    let _ = task.shutdown.send(());
                    task.handle.abort();
                }
                let file_transfer_state = state.file_transfer.0.clone();
                let file_transfer_cancel_state = state.file_transfer_cancel.0.clone();
                tauri::async_runtime::spawn(async move {
                    let mut guard = file_transfer_cancel_state.lock().await;
                    for (_, token) in guard.drain() {
                        token.cancel();
                    }
                    let mut guard = file_transfer_state.lock().await;
                    *guard = None;
                });
                let registry_pending = state.registry_pending.0.clone();
                tauri::async_runtime::spawn(async move {
                    registry_pending.lock().await.clear();
                });
                let remote_registry_pending = state.remote_registry_pending.0.clone();
                tauri::async_runtime::spawn(async move {
                    remote_registry_pending.lock().await.clear();
                });
                state.control.clear();
                state.registry_control.clear();
                if let Some(context) = state
                    .session_close
                    .0
                    .lock()
                    .ok()
                    .and_then(|mut guard| guard.take())
                {
                    tauri::async_runtime::spawn(notify_session_end(context));
                }
                if window.label().starts_with(SESSION_WINDOW_PREFIX) {
                    schedule_macos_accessory_if_no_session(window.app_handle().clone());
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            get_theme_preference,
            set_theme_preference,
            remember_update_api_base,
            viewer_check_for_updates,
            viewer_apply_staged_update,
            viewer_complete_update_exit_cleanup,
            test_button,
            connect_quic,
            connect_relay,
            disconnect_quic,
            disconnect_relay,
            registry_connect_quic,
            registry_connect_relay,
            registry_disconnect_quic,
            registry_disconnect_relay,
            clear_control_state,
            capture_remote_desktop_snapshot,
            viewport_set_rect,
            get_viewer_transport,
            send_control,
            get_launch_args,
            get_arg_dump,
            get_window_label,
            spawn_session_window,
            take_initial_url,
            set_start_menu_blocked,
            shell_connect,
            shell_connect_relay,
            shell_select_relay,
            shell_disconnect_relay,
            shell_connect_quic,
            shell_select_quic,
            shell_disconnect_quic,
            shell_write,
            shell_resize,
            shell_disconnect,
            file_transfer_connect,
            viewer_chat_connect,
            viewer_chat_send,
            viewer_chat_disconnect,
            file_transfer_disconnect,
            file_transfer_cancel,
            file_transfer_list_local,
            file_transfer_list_remote,
            file_transfer_remote_rename,
            file_transfer_remote_delete,
            file_transfer_local_rename,
            file_transfer_local_delete,
            file_transfer_upload,
            file_transfer_download,
            registry_list_keys,
            registry_list_values,
            registry_get_value,
            registry_set_value,
            registry_create_key,
            registry_delete_key,
            registry_delete_value
        ])
        .build(tauri::generate_context!())
        .expect("error while building Tauri application")
        .run(|app, event| {
            if let tauri::RunEvent::ExitRequested {
                code: None, api, ..
            } = event
            {
                if app.tray_by_id(TRAY_ID).is_some() {
                    info!("preventing viewer exit after all session windows closed");
                    api.prevent_exit();
                }
            }
        });
}
