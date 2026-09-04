#![cfg(target_os = "windows")]

use std::collections::VecDeque;
use std::mem;
use std::ptr;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use talos_protocol::{
    CONTROL_MOD_ALT, CONTROL_MOD_CTRL, CONTROL_MOD_SHIFT, CONTROL_MOD_WIN, CONTROL_PAYLOAD_KEY_LEN,
    CONTROL_PAYLOAD_MOUSE_BUTTON_LEN, CONTROL_PAYLOAD_MOUSE_DOUBLE_CLICK_LEN,
    CONTROL_PAYLOAD_MOUSE_MOVE_LEN, CONTROL_PAYLOAD_MOUSE_WHEEL_LEN,
    CONTROL_PAYLOAD_STREAM_BITRATE_LEN, CONTROL_TYPE_CLIPBOARD, CONTROL_TYPE_KEY_DOWN,
    CONTROL_TYPE_KEY_UP, CONTROL_TYPE_MOUSE_BUTTON, CONTROL_TYPE_MOUSE_DOUBLE_CLICK,
    CONTROL_TYPE_MOUSE_MOVE, CONTROL_TYPE_MOUSE_WHEEL, CONTROL_TYPE_STOP_CAPTURE,
    CONTROL_TYPE_STREAM_BITRATE, CONTROL_TYPE_TYPED_INPUT,
};
use tokio::sync::Notify;
use tracing::{debug, warn};
use winapi::shared::minwindef::{DWORD, HWINSTA, UINT};
use winapi::shared::winerror::ERROR_ACCESS_DENIED;
use winapi::um::errhandlingapi::GetLastError;
use winapi::um::processthreadsapi::{GetCurrentProcess, OpenProcessToken};
use winapi::um::securitybaseapi::AdjustTokenPrivileges;
use winapi::um::winbase::LookupPrivilegeValueW;
use winapi::um::winbase::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
use winapi::um::winnt::GENERIC_WRITE;
use winapi::um::winnt::{TOKEN_ADJUST_PRIVILEGES, TOKEN_PRIVILEGES, TOKEN_QUERY};
use winapi::um::winuser::{
    CloseClipboard, CloseDesktop, CloseWindowStation, EmptyClipboard, GetProcessWindowStation,
    GetThreadDesktop, GetUserObjectInformationW, OpenClipboard, OpenInputDesktop,
    OpenWindowStationW, SendInput, SetClipboardData, SetProcessWindowStation, SetThreadDesktop,
    CF_UNICODETEXT, DESKTOP_CREATEMENU, DESKTOP_CREATEWINDOW, DESKTOP_ENUMERATE,
    DESKTOP_HOOKCONTROL, DESKTOP_READOBJECTS, DESKTOP_SWITCHDESKTOP, DESKTOP_WRITEOBJECTS, INPUT,
    INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT, KEYEVENTF_KEYUP, KEYEVENTF_SCANCODE,
    KEYEVENTF_UNICODE, MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP,
    MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP, MOUSEEVENTF_MOVE, MOUSEEVENTF_RIGHTDOWN,
    MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_VIRTUALDESK, MOUSEEVENTF_WHEEL, SM_CMONITORS,
    SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN, UOI_NAME,
    VK_CONTROL, VK_LWIN, VK_MENU, VK_SHIFT, WINSTA_ALL_ACCESS,
};

#[derive(Debug, Clone)]
pub enum ControlMessage {
    MouseMove {
        x: u32,
        y: u32,
    },
    MouseButton {
        button: u8,
        down: bool,
        x: u32,
        y: u32,
    },
    MouseDoubleClick {
        button: u8,
        x: u32,
        y: u32,
    },
    MouseWheel {
        delta: i16,
        x: u32,
        y: u32,
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
    StreamBitrate {
        kbps: u32,
    },
    /// Agent → helper: stop capture loop (session closed). No payload.
    StopCapture,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DesktopContext {
    Default,
    Winlogon,
    SecureDesktop(String),
    Unknown,
}

static DESKTOP_CONTEXT_REFRESH_EPOCH: AtomicU64 = AtomicU64::new(1);

pub fn request_desktop_context_refresh() {
    let _ = DESKTOP_CONTEXT_REFRESH_EPOCH.fetch_add(1, Ordering::SeqCst);
}

pub fn desktop_context_refresh_epoch() -> u64 {
    DESKTOP_CONTEXT_REFRESH_EPOCH.load(Ordering::SeqCst)
}

/// Epoch counter incremented when a desktop transition requires helper process restart.
/// Helper processes launched with a specific desktop token (e.g. winlogon) cannot see
/// desktop switches via OpenInputDesktop—they must be killed and relaunched by the agent.
static PIPELINE_REBUILD_EPOCH: AtomicU64 = AtomicU64::new(0);

pub fn request_pipeline_rebuild() {
    PIPELINE_REBUILD_EPOCH.fetch_add(1, Ordering::SeqCst);
}

pub fn pipeline_rebuild_epoch() -> u64 {
    PIPELINE_REBUILD_EPOCH.load(Ordering::SeqCst)
}

/// Global DXGI capture output index for the active remote-desktop session (helper process).
/// Viewer sends pointer coords normalized to the **stream** (0..65535); `SendInput` with
/// `MOUSEEVENTF_ABSOLUTE` expects coords mapped to the **virtual desktop**. The helper
/// remaps using this index and `dxgi_output_desktop_rect_for_global_index`.
static REMOTE_INPUT_CAPTURE_OUTPUT_INDEX: AtomicUsize = AtomicUsize::new(0);

pub fn set_remote_input_capture_output_index(index: usize) {
    REMOTE_INPUT_CAPTURE_OUTPUT_INDEX.store(index, Ordering::Relaxed);
}

/// Convert viewer-normalized coords (full captured frame → 0..65535) into
/// virtual-desktop-normalized coords for `MOUSEEVENTF_ABSOLUTE`.
pub fn remap_remote_desktop_mouse_normalized(nx: u32, ny: u32) -> (u32, u32) {
    // One monitor: virtual desktop equals that display — the viewer already maps
    // the stream to 0..65535 in the same space `SendInput` expects. DXGI desktop
    // rects plus `GetSystemMetrics` can disagree slightly (DPI / rounding), which
    // visibly misaligns the cursor; skip remap unless the desktop is genuinely
    // extended across multiple monitors.
    let monitor_count = unsafe { winapi::um::winuser::GetSystemMetrics(SM_CMONITORS) };
    if monitor_count <= 1 {
        return (nx, ny);
    }

    let idx = REMOTE_INPUT_CAPTURE_OUTPUT_INDEX.load(Ordering::Relaxed);
    let Some((left, top, right, bottom)) =
        crate::capture::dxgi_output_desktop_rect_for_global_index(idx)
    else {
        return (nx, ny);
    };
    let mon_w = (right - left) as i64;
    let mon_h = (bottom - top) as i64;
    if mon_w <= 0 || mon_h <= 0 {
        return (nx, ny);
    }
    let px = left as i64 + (nx as i64 * (mon_w - 1).max(1)) / 65535;
    let py = top as i64 + (ny as i64 * (mon_h - 1).max(1)) / 65535;
    let vl = unsafe { winapi::um::winuser::GetSystemMetrics(SM_XVIRTUALSCREEN) } as i64;
    let vt = unsafe { winapi::um::winuser::GetSystemMetrics(SM_YVIRTUALSCREEN) } as i64;
    let vw = unsafe { winapi::um::winuser::GetSystemMetrics(SM_CXVIRTUALSCREEN) } as i64;
    let vh = unsafe { winapi::um::winuser::GetSystemMetrics(SM_CYVIRTUALSCREEN) } as i64;
    let denom_x = vw.saturating_sub(1).max(1);
    let denom_y = vh.saturating_sub(1).max(1);
    let out_x = ((px - vl).saturating_mul(65535) / denom_x).clamp(0, 65535) as u32;
    let out_y = ((py - vt).saturating_mul(65535) / denom_y).clamp(0, 65535) as u32;
    (out_x, out_y)
}

pub fn classify_desktop_name(name: &str) -> DesktopContext {
    if name.eq_ignore_ascii_case("default") {
        DesktopContext::Default
    } else if name.eq_ignore_ascii_case("winlogon") {
        DesktopContext::Winlogon
    } else if name.is_empty() {
        DesktopContext::Unknown
    } else {
        DesktopContext::SecureDesktop(name.to_string())
    }
}

pub fn input_desktop_context() -> DesktopContext {
    match input_desktop_name() {
        Some(name) => classify_desktop_name(&name),
        None => DesktopContext::Unknown,
    }
}

pub fn should_prefer_helper_injection(context: &DesktopContext) -> bool {
    !matches!(context, DesktopContext::Unknown)
}

#[cfg(target_os = "windows")]
fn enable_privilege(name: &str) {
    unsafe {
        let mut token = winapi::um::winnt::HANDLE::default();
        if OpenProcessToken(
            GetCurrentProcess(),
            TOKEN_QUERY | TOKEN_ADJUST_PRIVILEGES,
            &mut token,
        ) == 0
        {
            return;
        }
        let name_wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
        let mut luid = winapi::shared::ntdef::LUID {
            LowPart: 0,
            HighPart: 0,
        };
        if LookupPrivilegeValueW(std::ptr::null(), name_wide.as_ptr(), &mut luid) == 0 {
            return;
        }
        let mut tp: TOKEN_PRIVILEGES = std::mem::zeroed();
        tp.PrivilegeCount = 1;
        tp.Privileges[0].Luid = luid;
        tp.Privileges[0].Attributes = winapi::um::winnt::SE_PRIVILEGE_ENABLED;
        let _ = AdjustTokenPrivileges(
            token,
            0,
            &mut tp,
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        );
    }
}

#[cfg(target_os = "windows")]
pub fn input_desktop_name() -> Option<String> {
    unsafe {
        let desktop = OpenInputDesktop(
            0,
            0 as winapi::shared::minwindef::BOOL,
            DESKTOP_CREATEMENU
                | DESKTOP_CREATEWINDOW
                | DESKTOP_ENUMERATE
                | DESKTOP_HOOKCONTROL
                | DESKTOP_READOBJECTS
                | DESKTOP_WRITEOBJECTS
                | DESKTOP_SWITCHDESKTOP
                | GENERIC_WRITE,
        );
        if desktop.is_null() {
            return None;
        }
        let name = get_desktop_name(desktop);
        CloseDesktop(desktop);
        name
    }
}

pub fn parse_control_message(message_type: u8, payload: &[u8]) -> Option<ControlMessage> {
    match message_type {
        CONTROL_TYPE_MOUSE_MOVE => {
            if payload.len() != CONTROL_PAYLOAD_MOUSE_MOVE_LEN {
                return None;
            }
            let x = u32::from_be_bytes([payload[0], payload[1], payload[2], payload[3]]);
            let y = u32::from_be_bytes([payload[4], payload[5], payload[6], payload[7]]);
            Some(ControlMessage::MouseMove { x, y })
        }
        CONTROL_TYPE_MOUSE_BUTTON => {
            if payload.len() != CONTROL_PAYLOAD_MOUSE_BUTTON_LEN {
                return None;
            }
            let button = payload[0];
            let down = (payload[1] & 0x01) != 0;
            let x = u32::from_be_bytes([payload[2], payload[3], payload[4], payload[5]]);
            let y = u32::from_be_bytes([payload[6], payload[7], payload[8], payload[9]]);
            Some(ControlMessage::MouseButton { button, down, x, y })
        }
        CONTROL_TYPE_MOUSE_DOUBLE_CLICK => {
            if payload.len() != CONTROL_PAYLOAD_MOUSE_DOUBLE_CLICK_LEN {
                return None;
            }
            let button = payload[0];
            let x = u32::from_be_bytes([payload[1], payload[2], payload[3], payload[4]]);
            let y = u32::from_be_bytes([payload[5], payload[6], payload[7], payload[8]]);
            Some(ControlMessage::MouseDoubleClick { button, x, y })
        }
        CONTROL_TYPE_MOUSE_WHEEL => {
            if payload.len() != CONTROL_PAYLOAD_MOUSE_WHEEL_LEN {
                return None;
            }
            let delta = i16::from_be_bytes([payload[0], payload[1]]);
            let x = u32::from_be_bytes([payload[2], payload[3], payload[4], payload[5]]);
            let y = u32::from_be_bytes([payload[6], payload[7], payload[8], payload[9]]);
            Some(ControlMessage::MouseWheel { delta, x, y })
        }
        CONTROL_TYPE_KEY_DOWN => {
            if payload.len() != CONTROL_PAYLOAD_KEY_LEN {
                return None;
            }
            let vkey = u16::from_be_bytes([payload[0], payload[1]]);
            let scan = u16::from_be_bytes([payload[2], payload[3]]);
            let modifiers = payload[4];
            Some(ControlMessage::KeyDown {
                vkey,
                scan,
                modifiers,
            })
        }
        CONTROL_TYPE_KEY_UP => {
            if payload.len() != CONTROL_PAYLOAD_KEY_LEN {
                return None;
            }
            let vkey = u16::from_be_bytes([payload[0], payload[1]]);
            let scan = u16::from_be_bytes([payload[2], payload[3]]);
            let modifiers = payload[4];
            Some(ControlMessage::KeyUp {
                vkey,
                scan,
                modifiers,
            })
        }
        CONTROL_TYPE_CLIPBOARD | CONTROL_TYPE_TYPED_INPUT => {
            if payload.len() < 2 {
                return None;
            }
            let text_len = u16::from_be_bytes([payload[0], payload[1]]) as usize;
            if payload.len() != 2 + text_len {
                return None;
            }
            let text = std::str::from_utf8(&payload[2..])
                .ok()
                .map(|s| s.to_string())?;
            if message_type == CONTROL_TYPE_CLIPBOARD {
                Some(ControlMessage::Clipboard { text })
            } else {
                Some(ControlMessage::TypedInput { text })
            }
        }
        CONTROL_TYPE_STREAM_BITRATE => {
            if payload.len() != CONTROL_PAYLOAD_STREAM_BITRATE_LEN {
                return None;
            }
            let kbps = u32::from_be_bytes([payload[0], payload[1], payload[2], payload[3]]);
            Some(ControlMessage::StreamBitrate { kbps })
        }
        CONTROL_TYPE_STOP_CAPTURE => {
            if payload.is_empty() {
                Some(ControlMessage::StopCapture)
            } else {
                None
            }
        }
        _ => None,
    }
}

#[derive(Clone)]
pub struct ControlQueue {
    inner: Arc<ControlQueueInner>,
}

struct ControlQueueInner {
    queue: Mutex<VecDeque<ControlMessage>>,
    notify: Notify,
    capacity: usize,
}

impl ControlQueue {
    pub fn new(capacity: usize) -> Self {
        Self {
            inner: Arc::new(ControlQueueInner {
                queue: Mutex::new(VecDeque::with_capacity(capacity)),
                notify: Notify::new(),
                capacity,
            }),
        }
    }

    pub fn push(&self, message: ControlMessage) {
        let mut guard = match self.inner.queue.lock() {
            Ok(g) => g,
            Err(_) => return,
        };
        if guard.len() >= self.inner.capacity {
            if matches!(message, ControlMessage::MouseMove { .. }) {
                return;
            }
            if let Some(index) = guard
                .iter()
                .position(|item| matches!(item, ControlMessage::MouseMove { .. }))
            {
                guard.remove(index);
            } else {
                guard.pop_front();
            }
        }
        guard.push_back(message);
        self.inner.notify.notify_one();
    }

    async fn pop(&self) -> ControlMessage {
        loop {
            if let Ok(mut guard) = self.inner.queue.lock() {
                if let Some(message) = guard.pop_front() {
                    return message;
                }
            }
            self.inner.notify.notified().await;
        }
    }
}

pub async fn run_inject_loop(queue: ControlQueue) {
    loop {
        let message = queue.pop().await;
        if let Err(err) = handle_control_message(message) {
            warn!(error = %err, "control injection failed");
        }
    }
}

pub fn handle_control_message(message: ControlMessage) -> Result<(), String> {
    match message {
        ControlMessage::MouseMove { x, y } => {
            let (x, y) = remap_remote_desktop_mouse_normalized(x, y);
            send_mouse_move(x, y)
        }
        ControlMessage::MouseButton { button, down, x, y } => {
            let (x, y) = remap_remote_desktop_mouse_normalized(x, y);
            send_mouse_button(button, down, x, y)
        }
        ControlMessage::MouseDoubleClick { button, x, y } => {
            let (x, y) = remap_remote_desktop_mouse_normalized(x, y);
            send_mouse_double_click(button, x, y)
        }
        ControlMessage::MouseWheel { delta, x, y } => {
            let (x, y) = remap_remote_desktop_mouse_normalized(x, y);
            send_mouse_wheel(delta, x, y)
        }
        ControlMessage::KeyDown {
            vkey,
            scan,
            modifiers,
        } => send_key(vkey, scan, modifiers, true),
        ControlMessage::KeyUp {
            vkey,
            scan,
            modifiers,
        } => send_key(vkey, scan, modifiers, false),
        ControlMessage::Clipboard { text } => {
            set_clipboard_text(&text)?;
            send_key_combo_ctrl_v()
        }
        ControlMessage::TypedInput { text } => send_typed_input(&text),
        ControlMessage::StreamBitrate { .. } => Ok(()), // Handled in helper control loop.
        ControlMessage::StopCapture => Ok(()), // Handled in helper control loop; no input action.
    }
}

fn clamp_abs_coord(value: u32) -> i32 {
    value.min(65_535) as i32
}

/// Absolute mouse coords must include `MOUSEEVENTF_VIRTUALDESK` on extended desktops;
/// otherwise 0..65535 maps to the **primary** monitor only (SendInput / MSDN).
const MOUSEEVENTF_ABS_VIRTUAL: DWORD = MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK;

fn send_mouse_move(x: u32, y: u32) -> Result<(), String> {
    let input = build_mouse_input(MOUSEEVENTF_MOVE | MOUSEEVENTF_ABS_VIRTUAL, x, y, 0);
    send_inputs(&mut [input])
}

fn send_mouse_button(button: u8, down: bool, x: u32, y: u32) -> Result<(), String> {
    let Some(flags) = mouse_button_flags(button, down) else {
        return Ok(());
    };
    let input = build_mouse_input(flags | MOUSEEVENTF_ABS_VIRTUAL, x, y, 0);
    send_inputs(&mut [input])
}

fn send_mouse_double_click(button: u8, x: u32, y: u32) -> Result<(), String> {
    let Some(down_flags) = mouse_button_flags(button, true) else {
        return Ok(());
    };
    let Some(up_flags) = mouse_button_flags(button, false) else {
        return Ok(());
    };
    send_inputs(&mut [
        build_mouse_input(down_flags | MOUSEEVENTF_ABS_VIRTUAL, x, y, 0),
        build_mouse_input(up_flags | MOUSEEVENTF_ABS_VIRTUAL, x, y, 0),
        build_mouse_input(down_flags | MOUSEEVENTF_ABS_VIRTUAL, x, y, 0),
        build_mouse_input(up_flags | MOUSEEVENTF_ABS_VIRTUAL, x, y, 0),
    ])
}

fn mouse_button_flags(button: u8, down: bool) -> Option<DWORD> {
    match (button, down) {
        (0, true) => Some(MOUSEEVENTF_LEFTDOWN),
        (0, false) => Some(MOUSEEVENTF_LEFTUP),
        (1, true) => Some(MOUSEEVENTF_RIGHTDOWN),
        (1, false) => Some(MOUSEEVENTF_RIGHTUP),
        (2, true) => Some(MOUSEEVENTF_MIDDLEDOWN),
        (2, false) => Some(MOUSEEVENTF_MIDDLEUP),
        _ => None,
    }
}

fn send_mouse_wheel(delta: i16, x: u32, y: u32) -> Result<(), String> {
    if delta == 0 {
        return Ok(());
    }
    let move_input = build_mouse_input(MOUSEEVENTF_MOVE | MOUSEEVENTF_ABS_VIRTUAL, x, y, 0);
    let wheel_input = build_mouse_input(
        MOUSEEVENTF_WHEEL | MOUSEEVENTF_ABS_VIRTUAL,
        x,
        y,
        delta as i32,
    );
    send_inputs(&mut [move_input, wheel_input])
}

fn build_mouse_input(flags: DWORD, x: u32, y: u32, mouse_data: i32) -> INPUT {
    let mut input: INPUT = unsafe { mem::zeroed() };
    input.type_ = INPUT_MOUSE;
    unsafe {
        *input.u.mi_mut() = winapi::um::winuser::MOUSEINPUT {
            dx: clamp_abs_coord(x),
            dy: clamp_abs_coord(y),
            mouseData: mouse_data as DWORD,
            dwFlags: flags,
            time: 0,
            dwExtraInfo: 0,
        };
    }
    input
}

fn send_key(vkey: u16, scan: u16, modifiers: u8, is_down: bool) -> Result<(), String> {
    let mut inputs: Vec<INPUT> = Vec::new();
    if is_down {
        push_modifier_inputs(&mut inputs, modifiers, true);
    }
    inputs.push(build_key_input(vkey, scan, is_down));
    if !is_down {
        push_modifier_inputs(&mut inputs, modifiers, false);
    }
    send_inputs(&mut inputs)
}

fn push_modifier_inputs(inputs: &mut Vec<INPUT>, modifiers: u8, is_down: bool) {
    if modifiers & CONTROL_MOD_CTRL != 0 {
        inputs.push(build_key_input(VK_CONTROL as u16, 0, is_down));
    }
    if modifiers & CONTROL_MOD_SHIFT != 0 {
        inputs.push(build_key_input(VK_SHIFT as u16, 0, is_down));
    }
    if modifiers & CONTROL_MOD_ALT != 0 {
        inputs.push(build_key_input(VK_MENU as u16, 0, is_down));
    }
    if modifiers & CONTROL_MOD_WIN != 0 {
        inputs.push(build_key_input(VK_LWIN as u16, 0, is_down));
    }
}

fn build_key_input(vkey: u16, scan: u16, is_down: bool) -> INPUT {
    let mut input: INPUT = unsafe { mem::zeroed() };
    input.type_ = INPUT_KEYBOARD;
    let mut flags: DWORD = 0;
    if !is_down {
        flags |= KEYEVENTF_KEYUP;
    }
    if scan != 0 {
        flags |= KEYEVENTF_SCANCODE;
    }
    unsafe {
        *input.u.ki_mut() = KEYBDINPUT {
            wVk: if scan != 0 { 0 } else { vkey },
            wScan: scan,
            dwFlags: flags,
            time: 0,
            dwExtraInfo: 0,
        };
    }
    input
}

fn send_typed_input(text: &str) -> Result<(), String> {
    let mut inputs: Vec<INPUT> = Vec::new();
    for unit in text.encode_utf16() {
        inputs.push(build_unicode_input(unit, true));
        inputs.push(build_unicode_input(unit, false));
    }
    if inputs.is_empty() {
        return Ok(());
    }
    send_inputs(&mut inputs)
}

fn build_unicode_input(unit: u16, is_down: bool) -> INPUT {
    let mut input: INPUT = unsafe { mem::zeroed() };
    input.type_ = INPUT_KEYBOARD;
    let mut flags: DWORD = KEYEVENTF_UNICODE;
    if !is_down {
        flags |= KEYEVENTF_KEYUP;
    }
    unsafe {
        *input.u.ki_mut() = KEYBDINPUT {
            wVk: 0,
            wScan: unit,
            dwFlags: flags,
            time: 0,
            dwExtraInfo: 0,
        };
    }
    input
}

fn send_key_combo_ctrl_v() -> Result<(), String> {
    let mut inputs = vec![
        build_key_input(VK_CONTROL as u16, 0, true),
        build_key_input(0x56, 0, true),
        build_key_input(0x56, 0, false),
        build_key_input(VK_CONTROL as u16, 0, false),
    ];
    send_inputs(&mut inputs)
}

fn send_inputs(inputs: &mut [INPUT]) -> Result<(), String> {
    ensure_input_desktop();
    let sent = unsafe {
        SendInput(
            inputs.len() as UINT,
            inputs.as_mut_ptr(),
            mem::size_of::<INPUT>() as i32,
        )
    };
    if sent == inputs.len() as u32 {
        Ok(())
    } else {
        let err = unsafe { GetLastError() };
        Err(format!("SendInput failed: {}", err))
    }
}

#[cfg(target_os = "windows")]
fn ensure_input_desktop() {
    unsafe {
        let mut desktop = OpenInputDesktop(
            0,
            0 as winapi::shared::minwindef::BOOL,
            DESKTOP_CREATEMENU
                | DESKTOP_CREATEWINDOW
                | DESKTOP_ENUMERATE
                | DESKTOP_HOOKCONTROL
                | DESKTOP_READOBJECTS
                | DESKTOP_WRITEOBJECTS
                | DESKTOP_SWITCHDESKTOP
                | GENERIC_WRITE,
        );
        if desktop.is_null() {
            let err = GetLastError();
            if err == ERROR_ACCESS_DENIED {
                let current_winsta = get_winsta_name(GetProcessWindowStation())
                    .unwrap_or_else(|| "<none>".to_string());
                if !current_winsta.eq_ignore_ascii_case("WinSta0") {
                    enable_privilege("SeTcbPrivilege");
                    enable_privilege("SeDebugPrivilege");
                    let winsta0_name: Vec<u16> =
                        "WinSta0".encode_utf16().chain(std::iter::once(0)).collect();
                    let winsta0 = OpenWindowStationW(winsta0_name.as_ptr(), 0, WINSTA_ALL_ACCESS);
                    if !winsta0.is_null() {
                        // Attach process to WinSta0 so OpenInputDesktop can succeed.
                        let _ = SetProcessWindowStation(winsta0);
                        CloseWindowStation(winsta0);
                    }
                    desktop = OpenInputDesktop(
                        0,
                        0 as winapi::shared::minwindef::BOOL,
                        DESKTOP_CREATEMENU
                            | DESKTOP_CREATEWINDOW
                            | DESKTOP_ENUMERATE
                            | DESKTOP_HOOKCONTROL
                            | DESKTOP_READOBJECTS
                            | DESKTOP_WRITEOBJECTS
                            | DESKTOP_SWITCHDESKTOP
                            | GENERIC_WRITE,
                    );
                    if desktop.is_null() {
                        return;
                    }
                } else {
                    return;
                }
            } else {
                return;
            }
        }

        let current = GetThreadDesktop(winapi::um::processthreadsapi::GetCurrentThreadId());
        let needs_switch = match (get_desktop_name(current), get_desktop_name(desktop)) {
            (Some(current_name), Some(input_name)) => current_name != input_name,
            _ => true,
        };

        if needs_switch {
            let ok = SetThreadDesktop(desktop);
            if ok == 0 {
                let _ = GetLastError();
            }
        }

        CloseDesktop(desktop);
    }
}

#[cfg(target_os = "windows")]
fn get_desktop_name(desktop: winapi::shared::windef::HDESK) -> Option<String> {
    unsafe {
        let mut needed: DWORD = 0;
        let _ = GetUserObjectInformationW(
            desktop as *mut _,
            UOI_NAME as i32,
            std::ptr::null_mut(),
            0,
            &mut needed as *mut _,
        );
        if needed == 0 {
            return None;
        }
        let mut buf: Vec<u16> = vec![0; (needed as usize).div_ceil(2)];
        let ok = GetUserObjectInformationW(
            desktop as *mut _,
            UOI_NAME as i32,
            buf.as_mut_ptr() as *mut _,
            needed,
            &mut needed as *mut _,
        );
        if ok == 0 {
            return None;
        }
        let len = buf.iter().position(|c| *c == 0).unwrap_or(buf.len());
        Some(String::from_utf16_lossy(&buf[..len]))
    }
}

#[cfg(target_os = "windows")]
fn get_winsta_name(winsta: HWINSTA) -> Option<String> {
    unsafe {
        let mut needed: DWORD = 0;
        let _ = GetUserObjectInformationW(
            winsta as *mut _,
            UOI_NAME as i32,
            std::ptr::null_mut(),
            0,
            &mut needed as *mut _,
        );
        if needed == 0 {
            return None;
        }
        let mut buf: Vec<u16> = vec![0; (needed as usize).div_ceil(2)];
        let ok = GetUserObjectInformationW(
            winsta as *mut _,
            UOI_NAME as i32,
            buf.as_mut_ptr() as *mut _,
            needed,
            &mut needed as *mut _,
        );
        if ok == 0 {
            return None;
        }
        let len = buf.iter().position(|c| *c == 0).unwrap_or(buf.len());
        Some(String::from_utf16_lossy(&buf[..len]))
    }
}

/// Switch the calling thread to the current input desktop.
///
/// This is required before DXGI re-initialization after a desktop transition
/// (e.g. Winlogon → Default after user login, or Default → Winlogon for UAC).
/// Returns `true` if the thread was successfully attached, `false` otherwise.
pub fn attach_thread_to_input_desktop() -> bool {
    unsafe {
        use winapi::shared::minwindef::FALSE;
        use winapi::um::winnt::GENERIC_ALL;
        use winapi::um::winuser::{
            CloseDesktop, OpenInputDesktop, SetThreadDesktop, DESKTOP_CREATEWINDOW,
            DESKTOP_READOBJECTS, DESKTOP_SWITCHDESKTOP, DESKTOP_WRITEOBJECTS,
        };

        let desktop = OpenInputDesktop(
            0,
            FALSE,
            DESKTOP_CREATEWINDOW
                | DESKTOP_READOBJECTS
                | DESKTOP_WRITEOBJECTS
                | DESKTOP_SWITCHDESKTOP
                | GENERIC_ALL,
        );
        if desktop.is_null() {
            return false;
        }
        let ok = SetThreadDesktop(desktop);
        CloseDesktop(desktop);
        ok != 0
    }
}

fn set_clipboard_text(text: &str) -> Result<(), String> {
    let mut wide: Vec<u16> = text.encode_utf16().collect();
    wide.push(0);
    let bytes = wide.len() * mem::size_of::<u16>();
    unsafe {
        if OpenClipboard(ptr::null_mut()) == 0 {
            return Err("OpenClipboard failed".to_string());
        }
        if EmptyClipboard() == 0 {
            CloseClipboard();
            return Err("EmptyClipboard failed".to_string());
        }
        let handle = GlobalAlloc(GMEM_MOVEABLE, bytes);
        if handle.is_null() {
            CloseClipboard();
            return Err("GlobalAlloc failed".to_string());
        }
        let locked = GlobalLock(handle) as *mut u16;
        if locked.is_null() {
            CloseClipboard();
            return Err("GlobalLock failed".to_string());
        }
        ptr::copy_nonoverlapping(wide.as_ptr(), locked, wide.len());
        GlobalUnlock(handle);
        if SetClipboardData(CF_UNICODETEXT, handle).is_null() {
            CloseClipboard();
            return Err("SetClipboardData failed".to_string());
        }
        CloseClipboard();
    }
    debug!(len = text.len(), "clipboard updated");
    Ok(())
}
