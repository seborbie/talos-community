#[cfg(target_os = "windows")]
use std::sync::mpsc;
#[cfg(target_os = "windows")]
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
#[cfg(target_os = "windows")]
use talos_protocol::{
    CONTROL_PAYLOAD_CAPTURE_OUTPUT_INDEX_LEN, CONTROL_TYPE_CAPTURE_OUTPUT_SWITCH,
    HELPER_PIPE_HANDSHAKE_MAGIC, HELPER_PIPE_MAX_AUTH_TOKEN_LEN, HELPER_PIPE_PROTOCOL_VERSION,
    RMM_DISPLAY_PROCESSING_MODE_ENV,
};

#[cfg(target_os = "windows")]
mod atlas;
#[cfg(target_os = "windows")]
mod dump;
#[cfg(target_os = "windows")]
mod dxgi_atlas_dump;
#[cfg(target_os = "windows")]
mod dxgi_capture;
#[cfg(target_os = "windows")]
mod experimental_stream;
#[cfg(target_os = "windows")]
mod logging;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
mod macos_h264;
#[cfg(target_os = "windows")]
mod replay;
#[cfg(target_os = "windows")]
mod tile_commands;

#[cfg(target_os = "windows")]
fn helper_log_path() -> std::path::PathBuf {
    std::path::PathBuf::from(r"C:\ProgramData\Talos\logs\talos_worker_helper.log")
}

#[cfg(target_os = "windows")]
fn helper_log(msg: &str, data: Option<&str>) {
    use tracing::{debug, info, warn};
    const TARGET: &str = "talos_worker_helper";
    let warn = msg.contains("_err") || msg.contains("failed") || msg.contains("missing");
    let lifecycle = matches!(
        msg,
        "helper_started"
            | "helper_role"
            | "helper_identity_env"
            | "helper_rmm_session_id"
            | "helper_session_seq"
            | "helper_pipe_instance"
            | "helper_display_processing_mode"
            | "capture_pipeline_start"
            | "capture_pipeline_exit_ok"
            | "capture_pipeline_exit_err"
            | "helper_exited_ok"
            | "helper_exited_err"
            | "control_pipe_loop_start"
            | "control_pipe_start"
            | "dxgi_atlas_dump_start"
            | "dxgi_atlas_dump_exit_ok"
            | "process_priority_set"
            | "sas_triggered"
            | "sas_dispatch"
            | "sas_sent"
    );
    if warn {
        match data {
            Some(d) => warn!(target: TARGET, event = %msg, data = %d),
            None => warn!(target: TARGET, event = %msg),
        }
    } else if lifecycle {
        match data {
            Some(d) => info!(target: TARGET, event = %msg, data = %d),
            None => info!(target: TARGET, event = %msg),
        }
    } else {
        match data {
            Some(d) => debug!(target: TARGET, event = %msg, data = %d),
            None => debug!(target: TARGET, event = %msg),
        }
    }
}

#[cfg(target_os = "windows")]
fn enable_privilege(name: &str) {
    unsafe {
        use winapi::shared::minwindef::FALSE;
        use winapi::um::processthreadsapi::{GetCurrentProcess, OpenProcessToken};
        use winapi::um::securitybaseapi::AdjustTokenPrivileges;
        use winapi::um::winbase::LookupPrivilegeValueW;
        use winapi::um::winnt::{TOKEN_ADJUST_PRIVILEGES, TOKEN_PRIVILEGES, TOKEN_QUERY};

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
            FALSE,
            &mut tp,
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        );
    }
}

#[cfg(target_os = "windows")]
fn set_process_priority_above_normal() {
    unsafe {
        use winapi::um::errhandlingapi::GetLastError;
        use winapi::um::processthreadsapi::{GetCurrentProcess, SetPriorityClass};
        use winapi::um::winbase::ABOVE_NORMAL_PRIORITY_CLASS;

        let ok = SetPriorityClass(GetCurrentProcess(), ABOVE_NORMAL_PRIORITY_CLASS);
        if ok == 0 {
            let err = GetLastError();
            helper_log("process_priority_set_err", Some(&format!("{}", err)));
        } else {
            helper_log("process_priority_set", Some("ABOVE_NORMAL"));
        }
    }
}

#[cfg(target_os = "windows")]
fn open_named_pipe_reader(pipe_name: &str) -> anyhow::Result<winapi::um::winnt::HANDLE> {
    use winapi::um::errhandlingapi::GetLastError;
    use winapi::um::fileapi::{CreateFileW, OPEN_EXISTING};
    use winapi::um::handleapi::INVALID_HANDLE_VALUE;
    use winapi::um::winnt::{FILE_ATTRIBUTE_NORMAL, GENERIC_READ, GENERIC_WRITE};

    let pipe_wide: Vec<u16> = pipe_name.encode_utf16().chain(std::iter::once(0)).collect();
    for _attempt in 1..=60 {
        let handle = unsafe {
            CreateFileW(
                pipe_wide.as_ptr(),
                GENERIC_READ | GENERIC_WRITE,
                0,
                std::ptr::null_mut(),
                OPEN_EXISTING,
                FILE_ATTRIBUTE_NORMAL,
                std::ptr::null_mut(),
            )
        };
        if handle != INVALID_HANDLE_VALUE {
            return Ok(handle);
        }
        let err = unsafe { GetLastError() };
        if err == winapi::shared::winerror::ERROR_PIPE_BUSY
            || err == winapi::shared::winerror::ERROR_FILE_NOT_FOUND
        {
            std::thread::sleep(std::time::Duration::from_millis(100));
            continue;
        }
        anyhow::bail!("CreateFileW pipe failed: {}", err);
    }
    anyhow::bail!("timed out connecting to pipe");
}

#[cfg(target_os = "windows")]
fn read_pipe_exact(handle: winapi::um::winnt::HANDLE, buf: &mut [u8]) -> anyhow::Result<()> {
    use winapi::shared::minwindef::DWORD;
    use winapi::um::errhandlingapi::GetLastError;
    use winapi::um::fileapi::ReadFile;

    let mut offset: usize = 0;
    while offset < buf.len() {
        let mut read: DWORD = 0;
        let ok = unsafe {
            ReadFile(
                handle,
                buf[offset..].as_mut_ptr() as *mut _,
                (buf.len() - offset) as DWORD,
                &mut read,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 {
            let err = unsafe { GetLastError() };
            anyhow::bail!("ReadFile failed: {}", err);
        }
        if read == 0 {
            anyhow::bail!("ReadFile returned 0 bytes");
        }
        offset += read as usize;
    }
    Ok(())
}

#[cfg(target_os = "windows")]
fn write_pipe_all(handle: winapi::um::winnt::HANDLE, buf: &[u8]) -> anyhow::Result<()> {
    use winapi::shared::minwindef::DWORD;
    use winapi::um::errhandlingapi::GetLastError;
    use winapi::um::fileapi::WriteFile;

    let mut offset: usize = 0;
    while offset < buf.len() {
        let mut written: DWORD = 0;
        let ok = unsafe {
            WriteFile(
                handle,
                buf[offset..].as_ptr() as *const _,
                (buf.len() - offset) as DWORD,
                &mut written,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 {
            let err = unsafe { GetLastError() };
            anyhow::bail!("WriteFile failed: {}", err);
        }
        if written == 0 {
            anyhow::bail!("WriteFile wrote 0 bytes");
        }
        offset += written as usize;
    }
    Ok(())
}

#[cfg(target_os = "windows")]
fn control_pipe_loop(
    pipe_name: String,
    auth_token: String,
    stop: Arc<AtomicBool>,
    capture_output_switch_tx: Option<mpsc::Sender<usize>>,
    stream_bitrate_tx: Option<mpsc::Sender<u32>>,
) {
    helper_log("control_pipe_start", Some(&pipe_name));
    let handle = match open_named_pipe_reader(&pipe_name) {
        Ok(handle) => handle,
        Err(err) => {
            helper_log("control_pipe_open_err", Some(&format!("{}", err)));
            stop.store(true, Ordering::SeqCst);
            return;
        }
    };

    // Authenticate to the agent over the duplex control pipe.
    if auth_token.is_empty() || auth_token.len() > HELPER_PIPE_MAX_AUTH_TOKEN_LEN {
        helper_log("control_pipe_auth_err", Some("invalid auth token"));
        stop.store(true, Ordering::SeqCst);
        return;
    }
    let mut handshake = Vec::with_capacity(4 + 2 + 2 + auth_token.len());
    handshake.extend_from_slice(&HELPER_PIPE_HANDSHAKE_MAGIC);
    handshake.extend_from_slice(&HELPER_PIPE_PROTOCOL_VERSION.to_be_bytes());
    handshake.extend_from_slice(&(auth_token.len() as u16).to_be_bytes());
    handshake.extend_from_slice(auth_token.as_bytes());
    if let Err(err) = write_pipe_all(handle, &handshake) {
        helper_log("control_pipe_auth_err", Some(&format!("{}", err)));
        stop.store(true, Ordering::SeqCst);
        return;
    }

    let (input_tx, input_rx) = mpsc::channel::<talos_worker::control::ControlMessage>();
    std::thread::spawn(move || clean_input_loop(input_rx));
    loop {
        let mut len_buf = [0u8; 2];
        if read_pipe_exact(handle, &mut len_buf).is_err() {
            helper_log("control_pipe_read_len_err", None);
            stop.store(true, Ordering::SeqCst);
            break;
        }
        let payload_len = u16::from_be_bytes(len_buf) as usize;
        let mut type_buf = [0u8; 1];
        if read_pipe_exact(handle, &mut type_buf).is_err() {
            helper_log("control_pipe_read_type_err", None);
            stop.store(true, Ordering::SeqCst);
            break;
        }
        let mut payload = vec![0u8; payload_len];
        if payload_len > 0 && read_pipe_exact(handle, &mut payload).is_err() {
            helper_log("control_pipe_read_payload_err", None);
            stop.store(true, Ordering::SeqCst);
            break;
        }
        if type_buf[0] == CONTROL_TYPE_CAPTURE_OUTPUT_SWITCH {
            if payload.len() == CONTROL_PAYLOAD_CAPTURE_OUTPUT_INDEX_LEN {
                let idx =
                    u32::from_be_bytes([payload[0], payload[1], payload[2], payload[3]]) as usize;
                if let Some(tx) = capture_output_switch_tx.as_ref() {
                    let _ = tx.send(idx);
                }
            }
            continue;
        }
        if let Some(message) = talos_worker::control::parse_control_message(type_buf[0], &payload) {
            match &message {
                talos_worker::control::ControlMessage::StopCapture => {
                    stop.store(true, Ordering::SeqCst);
                    helper_log("stop_capture_received", None);
                    continue;
                }
                talos_worker::control::ControlMessage::StreamBitrate { kbps } => {
                    if let Some(tx) = stream_bitrate_tx.as_ref() {
                        let _ = tx.send(*kbps);
                        helper_log("stream_bitrate_received", Some(&format!("kbps={kbps}")));
                    }
                    continue;
                }
                talos_worker::control::ControlMessage::KeyDown {
                    vkey, modifiers, ..
                } => {
                    let ctrl = (modifiers & talos_protocol::CONTROL_MOD_CTRL) != 0;
                    let shift = (modifiers & talos_protocol::CONTROL_MOD_SHIFT) != 0;
                    if ctrl && shift && *vkey == winapi::um::winuser::VK_DELETE as u16 {
                        helper_log("sas_triggered", None);
                        let _ = send_sas();
                        continue;
                    }
                }
                talos_worker::control::ControlMessage::Clipboard { .. } => {
                    if let Err(err) = talos_worker::control::handle_control_message(message) {
                        helper_log("control_inject_err", Some(&err));
                    }
                    continue;
                }
                _ => {}
            }
            let should_log = should_log_control_message(&message);
            let description = if should_log {
                Some(describe_control_message(&message))
            } else {
                None
            };
            if let Some(description) = description.as_deref() {
                helper_log("control_message_received", Some(description));
            }
            if input_tx.send(message).is_err() {
                let detail = description
                    .as_deref()
                    .map(|description| format!("input thread closed {description}"))
                    .unwrap_or_else(|| "input thread closed".to_string());
                helper_log("control_inject_err", Some(&detail));
            }
        } else {
            helper_log("control_pipe_parse_err", None);
        }
    }
}

#[cfg(target_os = "windows")]
fn spawn_desktop_transition_monitor() {
    std::thread::spawn(|| {
        let mut last_name: Option<String> = None;
        loop {
            let current_name = get_input_desktop_name();
            if current_name != last_name {
                let from = last_name.clone().unwrap_or_else(|| "<none>".to_string());
                let to = current_name.clone().unwrap_or_else(|| "<none>".to_string());
                helper_log(
                    "desktop_context_changed",
                    Some(&format!("{} -> {}", from, to)),
                );
                last_name = current_name;
            }
            std::thread::sleep(std::time::Duration::from_millis(150));
        }
    });
}

#[cfg(target_os = "windows")]
fn describe_control_message(message: &talos_worker::control::ControlMessage) -> String {
    match message {
        talos_worker::control::ControlMessage::MouseMove { x, y } => {
            format!("kind=mouse_move x={x} y={y}")
        }
        talos_worker::control::ControlMessage::MouseButton { button, down, x, y } => {
            format!("kind=mouse_button button={button} down={down} x={x} y={y}")
        }
        talos_worker::control::ControlMessage::MouseDoubleClick { button, x, y } => {
            format!("kind=mouse_double_click button={button} x={x} y={y}")
        }
        talos_worker::control::ControlMessage::MouseWheel { delta, x, y } => {
            format!("kind=mouse_wheel delta={delta} x={x} y={y}")
        }
        talos_worker::control::ControlMessage::KeyDown {
            vkey,
            scan,
            modifiers,
        } => format!("kind=key_down vkey={vkey} scan={scan} modifiers={modifiers}"),
        talos_worker::control::ControlMessage::KeyUp {
            vkey,
            scan,
            modifiers,
        } => format!("kind=key_up vkey={vkey} scan={scan} modifiers={modifiers}"),
        talos_worker::control::ControlMessage::TypedInput { text } => {
            format!("kind=typed_input chars={}", text.chars().count())
        }
        talos_worker::control::ControlMessage::Clipboard { text } => {
            format!("kind=clipboard bytes={}", text.len())
        }
        talos_worker::control::ControlMessage::StreamBitrate { kbps } => {
            format!("kind=stream_bitrate kbps={kbps}")
        }
        talos_worker::control::ControlMessage::StopCapture => "kind=stop_capture".to_string(),
    }
}

#[cfg(target_os = "windows")]
fn should_log_control_message(message: &talos_worker::control::ControlMessage) -> bool {
    !matches!(
        message,
        talos_worker::control::ControlMessage::MouseMove { .. }
    )
}

#[cfg(target_os = "windows")]
fn clean_input_loop(rx: std::sync::mpsc::Receiver<talos_worker::control::ControlMessage>) {
    for message in rx {
        let should_log = should_log_control_message(&message);
        let description = if should_log {
            Some(describe_control_message(&message))
        } else {
            None
        };
        match inject_control_message_clean(message) {
            Ok(()) => {
                if let Some(description) = description.as_deref() {
                    helper_log("control_inject_ok", Some(description));
                }
            }
            Err(err) => {
                let detail = description
                    .as_deref()
                    .map(|description| format!("{description} error={err}"))
                    .unwrap_or(err);
                helper_log("control_inject_err", Some(&detail));
            }
        }
    }
}

#[cfg(target_os = "windows")]
fn attach_input_desktop() -> Result<(), String> {
    use winapi::shared::minwindef::FALSE;
    use winapi::um::errhandlingapi::GetLastError;
    use winapi::um::winnt::GENERIC_WRITE;
    use winapi::um::winuser::{
        CloseDesktop, OpenInputDesktop, SetThreadDesktop, DESKTOP_CREATEMENU, DESKTOP_CREATEWINDOW,
        DESKTOP_ENUMERATE, DESKTOP_HOOKCONTROL, DESKTOP_READOBJECTS, DESKTOP_SWITCHDESKTOP,
        DESKTOP_WRITEOBJECTS,
    };

    unsafe {
        let desktop = OpenInputDesktop(
            0,
            FALSE,
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
            return Err(format!("OpenInputDesktop failed: {}", err));
        }
        if SetThreadDesktop(desktop) == 0 {
            let err = GetLastError();
            CloseDesktop(desktop);
            return Err(format!("SetThreadDesktop failed: {}", err));
        }
        CloseDesktop(desktop);
    }
    Ok(())
}

#[cfg(target_os = "windows")]
fn inject_control_message_clean(
    message: talos_worker::control::ControlMessage,
) -> Result<(), String> {
    use talos_protocol::{CONTROL_MOD_ALT, CONTROL_MOD_CTRL, CONTROL_MOD_SHIFT, CONTROL_MOD_WIN};
    use winapi::shared::minwindef::DWORD;
    use winapi::um::errhandlingapi::GetLastError;
    use winapi::um::winuser::{
        SendInput, INPUT, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT, KEYEVENTF_KEYUP,
        KEYEVENTF_SCANCODE, KEYEVENTF_UNICODE, MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_LEFTDOWN,
        MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP, MOUSEEVENTF_MOVE,
        MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_VIRTUALDESK, MOUSEEVENTF_WHEEL,
        VK_CONTROL, VK_LWIN, VK_MENU, VK_SHIFT,
    };

    fn clamp_abs_coord(value: u32) -> i32 {
        value.min(65_535) as i32
    }

    const MOUSEEVENTF_ABS_VIRTUAL: DWORD = MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK;

    fn build_mouse_input(flags: DWORD, x: u32, y: u32, mouse_data: i32) -> INPUT {
        let mut input: INPUT = unsafe { std::mem::zeroed() };
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

    fn build_key_input(vkey: u16, scan: u16, is_down: bool) -> INPUT {
        let mut input: INPUT = unsafe { std::mem::zeroed() };
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

    fn build_unicode_input(unit: u16, is_down: bool) -> INPUT {
        let mut input: INPUT = unsafe { std::mem::zeroed() };
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

    fn send_inputs(inputs: &mut [INPUT]) -> Result<(), String> {
        let sent = unsafe {
            SendInput(
                inputs.len() as u32,
                inputs.as_mut_ptr(),
                std::mem::size_of::<INPUT>() as i32,
            )
        };
        if sent == inputs.len() as u32 {
            Ok(())
        } else {
            Err(format!("SendInput failed: {}", unsafe { GetLastError() }))
        }
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

    attach_input_desktop()?;

    match message {
        talos_worker::control::ControlMessage::MouseMove { x, y } => {
            let (x, y) = talos_worker::control::remap_remote_desktop_mouse_normalized(x, y);
            let input = build_mouse_input(MOUSEEVENTF_MOVE | MOUSEEVENTF_ABS_VIRTUAL, x, y, 0);
            send_inputs(&mut [input])
        }
        talos_worker::control::ControlMessage::MouseButton { button, down, x, y } => {
            let (x, y) = talos_worker::control::remap_remote_desktop_mouse_normalized(x, y);
            let flags = match (button, down) {
                (0, true) => MOUSEEVENTF_LEFTDOWN,
                (0, false) => MOUSEEVENTF_LEFTUP,
                (1, true) => MOUSEEVENTF_RIGHTDOWN,
                (1, false) => MOUSEEVENTF_RIGHTUP,
                (2, true) => MOUSEEVENTF_MIDDLEDOWN,
                (2, false) => MOUSEEVENTF_MIDDLEUP,
                _ => return Ok(()),
            };
            let input = build_mouse_input(flags | MOUSEEVENTF_ABS_VIRTUAL, x, y, 0);
            send_inputs(&mut [input])
        }
        talos_worker::control::ControlMessage::MouseDoubleClick { button, x, y } => {
            let (x, y) = talos_worker::control::remap_remote_desktop_mouse_normalized(x, y);
            let down_flags = match button {
                0 => MOUSEEVENTF_LEFTDOWN,
                1 => MOUSEEVENTF_RIGHTDOWN,
                2 => MOUSEEVENTF_MIDDLEDOWN,
                _ => return Ok(()),
            };
            let up_flags = match button {
                0 => MOUSEEVENTF_LEFTUP,
                1 => MOUSEEVENTF_RIGHTUP,
                2 => MOUSEEVENTF_MIDDLEUP,
                _ => return Ok(()),
            };
            send_inputs(&mut [
                build_mouse_input(down_flags | MOUSEEVENTF_ABS_VIRTUAL, x, y, 0),
                build_mouse_input(up_flags | MOUSEEVENTF_ABS_VIRTUAL, x, y, 0),
                build_mouse_input(down_flags | MOUSEEVENTF_ABS_VIRTUAL, x, y, 0),
                build_mouse_input(up_flags | MOUSEEVENTF_ABS_VIRTUAL, x, y, 0),
            ])
        }
        talos_worker::control::ControlMessage::MouseWheel { delta, x, y } => {
            if delta == 0 {
                return Ok(());
            }
            let (x, y) = talos_worker::control::remap_remote_desktop_mouse_normalized(x, y);
            let move_input = build_mouse_input(MOUSEEVENTF_MOVE | MOUSEEVENTF_ABS_VIRTUAL, x, y, 0);
            let wheel_input = build_mouse_input(
                MOUSEEVENTF_WHEEL | MOUSEEVENTF_ABS_VIRTUAL,
                x,
                y,
                delta as i32,
            );
            send_inputs(&mut [move_input, wheel_input])
        }
        talos_worker::control::ControlMessage::KeyDown {
            vkey,
            scan,
            modifiers,
        } => {
            let mut inputs: Vec<INPUT> = Vec::new();
            push_modifier_inputs(&mut inputs, modifiers, true);
            inputs.push(build_key_input(vkey, scan, true));
            send_inputs(&mut inputs)
        }
        talos_worker::control::ControlMessage::KeyUp {
            vkey,
            scan,
            modifiers,
        } => {
            let mut inputs: Vec<INPUT> = Vec::new();
            inputs.push(build_key_input(vkey, scan, false));
            push_modifier_inputs(&mut inputs, modifiers, false);
            send_inputs(&mut inputs)
        }
        talos_worker::control::ControlMessage::TypedInput { text } => {
            let mut inputs: Vec<INPUT> = Vec::new();
            for unit in text.encode_utf16() {
                inputs.push(build_unicode_input(unit, true));
                inputs.push(build_unicode_input(unit, false));
            }
            if inputs.is_empty() {
                Ok(())
            } else {
                send_inputs(&mut inputs)
            }
        }
        talos_worker::control::ControlMessage::Clipboard { .. } => Ok(()),
        talos_worker::control::ControlMessage::StreamBitrate { .. } => Ok(()),
        talos_worker::control::ControlMessage::StopCapture => Ok(()), // Handled in control_pipe_loop; not sent to input.
    }
}

#[cfg(target_os = "windows")]
fn get_input_desktop_name() -> Option<String> {
    use winapi::shared::minwindef::{DWORD, FALSE};
    use winapi::um::winuser::{
        CloseDesktop, GetUserObjectInformationW, OpenInputDesktop, DESKTOP_READOBJECTS, UOI_NAME,
    };

    unsafe {
        let desktop = OpenInputDesktop(0, FALSE, DESKTOP_READOBJECTS);
        if desktop.is_null() {
            return None;
        }
        let mut needed: DWORD = 0;
        let _ = GetUserObjectInformationW(
            desktop as *mut _,
            UOI_NAME as i32,
            std::ptr::null_mut(),
            0,
            &mut needed as *mut _,
        );
        if needed > 0 {
            let mut buf: Vec<u16> = vec![0; (needed as usize).div_ceil(2)];
            let ok = GetUserObjectInformationW(
                desktop as *mut _,
                UOI_NAME as i32,
                buf.as_mut_ptr() as *mut _,
                needed,
                &mut needed as *mut _,
            );
            if ok != 0 {
                let len = buf.iter().position(|c| *c == 0).unwrap_or(buf.len());
                let name = String::from_utf16_lossy(&buf[..len]);
                let _ = CloseDesktop(desktop);
                return Some(name);
            }
        }
        let _ = CloseDesktop(desktop);
    }
    None
}

#[cfg(target_os = "windows")]
fn send_sas() -> bool {
    use winapi::shared::minwindef::FALSE;
    use winapi::um::handleapi::CloseHandle;
    use winapi::um::processthreadsapi::{GetCurrentProcess, OpenProcessToken};
    use winapi::um::securitybaseapi::{CreateWellKnownSid, EqualSid, GetTokenInformation};
    use winapi::um::winnt::{
        TokenUser, WinLocalSystemSid, PSID, SECURITY_MAX_SID_SIZE, TOKEN_QUERY, TOKEN_USER,
    };

    #[link(name = "sas")]
    extern "system" {
        fn SendSAS(as_user: i32);
    }

    fn should_send_as_user() -> Option<bool> {
        unsafe {
            let mut token = std::ptr::null_mut();
            if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
                return None;
            }

            let mut needed = 0u32;
            let _ = GetTokenInformation(token, TokenUser, std::ptr::null_mut(), 0, &mut needed);
            if needed == 0 {
                let _ = CloseHandle(token);
                return None;
            }

            let mut token_buf = vec![0u8; needed as usize];
            let token_ok = GetTokenInformation(
                token,
                TokenUser,
                token_buf.as_mut_ptr() as *mut _,
                needed,
                &mut needed,
            );
            if token_ok == 0 {
                let _ = CloseHandle(token);
                return None;
            }

            let token_user = &*(token_buf.as_ptr() as *const TOKEN_USER);
            let mut local_system_sid = [0u8; SECURITY_MAX_SID_SIZE];
            let mut sid_len = local_system_sid.len() as u32;
            let sid_ok = CreateWellKnownSid(
                WinLocalSystemSid,
                std::ptr::null_mut(),
                local_system_sid.as_mut_ptr() as PSID,
                &mut sid_len,
            );
            let result = if sid_ok == 0 {
                None
            } else {
                let is_local_system =
                    EqualSid(token_user.User.Sid, local_system_sid.as_mut_ptr() as PSID) != 0;
                Some(!is_local_system)
            };
            let _ = CloseHandle(token);
            result
        }
    }

    let as_user = should_send_as_user().unwrap_or(false);
    helper_log(
        "sas_dispatch",
        Some(if as_user {
            "as_user=true"
        } else {
            "as_user=false"
        }),
    );
    unsafe {
        SendSAS(if as_user { 1 } else { FALSE });
    }
    helper_log(
        "sas_sent",
        Some(if as_user {
            "as_user=true"
        } else {
            "as_user=false"
        }),
    );
    true
}

#[cfg(target_os = "windows")]
fn strip_legacy_debug_env_vars() {
    std::env::remove_var("RUST_LOG");
    std::env::remove_var("RMM_AGENT");
}

/// When the helper is started with a console (e.g. double-click or `cmd`), the window otherwise
/// closes immediately after exit. Wait for a key so logs and `eprintln!` output can be read.
/// Skipped when there is no console window, when stdin is not the console, or when
/// `RMM_HELPER_NO_PAUSE_ON_EXIT` is set to `1` / `true` / `yes` (e.g. child of the agent).
#[cfg(target_os = "windows")]
fn pause_console_before_exit() {
    use std::io::Write;
    use winapi::um::consoleapi::ReadConsoleInputW;
    use winapi::um::fileapi::GetFileType;
    use winapi::um::handleapi::INVALID_HANDLE_VALUE;
    use winapi::um::processenv::GetStdHandle;
    use winapi::um::winbase::{FILE_TYPE_CHAR, STD_INPUT_HANDLE};
    use winapi::um::wincon::GetConsoleWindow;
    use winapi::um::wincontypes::{INPUT_RECORD, KEY_EVENT};
    use winapi::um::winnt::HANDLE;

    if std::env::var_os("RMM_HELPER_NO_PAUSE_ON_EXIT").is_some_and(|v| {
        let s = v.to_string_lossy();
        s == "1" || s.eq_ignore_ascii_case("true") || s.eq_ignore_ascii_case("yes")
    }) {
        return;
    }

    unsafe {
        if GetConsoleWindow().is_null() {
            return;
        }
        let h: HANDLE = GetStdHandle(STD_INPUT_HANDLE);
        if h.is_null() || h == INVALID_HANDLE_VALUE {
            return;
        }
        if GetFileType(h) != FILE_TYPE_CHAR {
            return;
        }

        let _ = writeln!(std::io::stderr(), "\nPress any key to close this window...");
        let _ = std::io::stderr().flush();
        let _ = std::io::stdout().flush();

        loop {
            let mut rec: INPUT_RECORD = std::mem::zeroed();
            let mut n: u32 = 0;
            if ReadConsoleInputW(h, &mut rec, 1, &mut n) == 0 {
                break;
            }
            if n != 1 {
                continue;
            }
            if rec.EventType == KEY_EVENT {
                let ke = *rec.Event.KeyEvent();
                if ke.bKeyDown != 0 {
                    break;
                }
            }
        }
    }
}

#[cfg(target_os = "windows")]
fn run_helper() -> anyhow::Result<()> {
    strip_legacy_debug_env_vars();
    logging::init_helper_tracing(helper_log_path());
    tracing::info!(
        target: "talos_worker_helper",
        log_path = %helper_log_path().display(),
        filter = %talos_protocol::rmm_tracing_filter_directive(),
        "Talos Worker helper tracing to log file and stderr (RMM_LOGLEVEL=info: lifecycle + target `rmm_media_foundation`; debug: DXGI/pipe/MF teardown detail)"
    );
    // Log panics to the same log file so we can see if the helper crashes.
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let template = helper_log_path();
        let _ = (|| -> std::io::Result<()> {
            use std::io::Write;
            let mut f = talos_log_util::open_today_log_append(&template)?;
            f.write_all(b"helper_panic\n")
        })();
        default_hook(info);
    }));
    let raw_args: Vec<String> = std::env::args().skip(1).collect();
    #[cfg(target_os = "windows")]
    {
        enable_privilege("SeTcbPrivilege");
        enable_privilege("SeDebugPrivilege");
        set_process_priority_above_normal();
    }
    if raw_args
        .first()
        .is_some_and(|arg| arg == "capture-dxgi-atlas-dump")
    {
        let options = dxgi_atlas_dump::DumpOptions::parse(&raw_args[1..])?;
        helper_log(
            "dxgi_atlas_dump_start",
            Some(&format!(
                "output={} frames={} output_index={}",
                options.output.display(),
                options.frames,
                options.capture_output_index
            )),
        );
        let start = std::time::Instant::now();
        dxgi_atlas_dump::run(options)?;
        helper_log(
            "dxgi_atlas_dump_exit_ok",
            Some(&format!("elapsed_ms={}", start.elapsed().as_millis())),
        );
        return Ok(());
    }
    let mut args = raw_args.into_iter();
    let mut pipe_name: Option<String> = None;
    let mut control_pipe_name: Option<String> = None;
    let mut auth_token: Option<String> = None;
    let mut rmm_session_id: Option<String> = None;
    let mut session_seq: Option<u64> = None;
    let mut pipe_instance: Option<u64> = None;
    let mut display_stream_mode: Option<String> = None;
    let mut display_processing_mode: Option<String> = None;
    let mut input_only = false;
    while let Some(arg) = args.next() {
        if arg == "--pipe" {
            pipe_name = args.next();
        } else if arg == "--control-pipe" {
            control_pipe_name = args.next();
        } else if arg == "--auth" {
            auth_token = args.next();
        } else if arg == "--rmm-session-id" {
            rmm_session_id = args.next();
        } else if arg == "--session-seq" {
            session_seq = args.next().and_then(|v| v.parse::<u64>().ok());
        } else if arg == "--pipe-instance" {
            pipe_instance = args.next().and_then(|v| v.parse::<u64>().ok());
        } else if arg == "--display-stream-mode" {
            display_stream_mode = args.next();
        } else if arg == "--display-processing-mode" {
            display_processing_mode = args.next();
        } else if arg == "--input-only" {
            input_only = true;
        }
    }

    if let Some(ref m) = display_processing_mode {
        let t = m.trim();
        if !t.is_empty() {
            // Must run before any `effective_display_processing_mode` use (inherited service env
            // is not passed through CreateProcessAsUserW with a NULL environment block).
            std::env::set_var(RMM_DISPLAY_PROCESSING_MODE_ENV, t);
            helper_log("helper_display_processing_mode", Some(t));
        }
    }

    let auth_token = auth_token
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| {
            eprintln!("missing --auth argument");
            pause_console_before_exit();
            std::process::exit(2);
        });

    if input_only && control_pipe_name.is_none() {
        eprintln!("missing --control-pipe argument for input-only");
        pause_console_before_exit();
        std::process::exit(2);
    }
    if !input_only && pipe_name.is_none() {
        eprintln!("missing --pipe argument");
        pause_console_before_exit();
        std::process::exit(2);
    }

    let helper_label = if input_only {
        control_pipe_name
            .clone()
            .unwrap_or_else(|| "<no-control-pipe>".to_string())
    } else {
        pipe_name.clone().unwrap_or_else(|| "<no-pipe>".to_string())
    };
    let helper_role = if input_only { "control" } else { "capture" };
    helper_log("helper_started", Some(&helper_label));
    helper_log("helper_role", Some(helper_role));
    if let Some(id) = rmm_session_id.clone().filter(|v| !v.trim().is_empty()) {
        // Propagate these into env vars so lower-level capture/encode code can include them in logs
        // without changing call signatures. Avoid logging auth token.
        std::env::set_var("RMM_HELPER_RMM_SESSION_ID", &id);
        helper_log("helper_rmm_session_id", Some(&id));
    }
    if let Some(seq) = session_seq {
        std::env::set_var("RMM_HELPER_SESSION_SEQ", format!("{}", seq));
        helper_log("helper_session_seq", Some(&format!("{}", seq)));
    }
    if let Some(inst) = pipe_instance {
        std::env::set_var("RMM_HELPER_PIPE_INSTANCE", format!("{}", inst));
        helper_log("helper_pipe_instance", Some(&format!("{}", inst)));
    }
    let helper_user = std::env::var("USERNAME").unwrap_or_else(|_| "<unknown>".to_string());
    let helper_domain = std::env::var("USERDOMAIN").unwrap_or_else(|_| "<unknown>".to_string());
    helper_log(
        "helper_identity_env",
        Some(&format!("{}\\{}", helper_domain, helper_user)),
    );
    spawn_desktop_transition_monitor();
    let stop = Arc::new(AtomicBool::new(false));
    let (capture_output_switch_tx, capture_output_switch_rx) = mpsc::channel::<usize>();
    let (stream_bitrate_tx, stream_bitrate_rx) = mpsc::channel::<u32>();
    if let Some(control_pipe) = control_pipe_name {
        if input_only {
            helper_log("control_pipe_loop_start", Some(&control_pipe));
            drop(capture_output_switch_tx);
            drop(stream_bitrate_tx);
            control_pipe_loop(control_pipe, auth_token, stop, None, None);
            helper_log("helper_exited_ok", Some(helper_role));
            return Ok(());
        }
        let auth_for_control = auth_token.clone();
        let stop_for_control = stop.clone();
        let control_pipe_label = control_pipe.clone();
        let switch_tx = capture_output_switch_tx;
        let bitrate_tx = stream_bitrate_tx;
        std::thread::spawn(move || {
            helper_log("control_pipe_loop_start", Some(&control_pipe_label));
            control_pipe_loop(
                control_pipe,
                auth_for_control,
                stop_for_control,
                Some(switch_tx),
                Some(bitrate_tx),
            );
        });
    } else {
        drop(capture_output_switch_tx);
        drop(stream_bitrate_tx);
    }

    if input_only {
        helper_log("helper_exited_ok", Some(helper_role));
        return Ok(());
    }

    let pipe_name = pipe_name.unwrap();
    let tuning = talos_worker::encode::load_encode_tuning_from_env();
    helper_log("capture_pipeline_start", Some(&pipe_name));
    let start = std::time::Instant::now();
    let experimental_mode = display_processing_mode.as_deref().is_some_and(|value| {
        let value = value.trim();
        value.eq_ignore_ascii_case("experimental")
    });
    if experimental_mode {
        match experimental_stream::run_experimental_stream_to_pipe(
            &pipe_name,
            &auth_token,
            tuning,
            30,
            stop,
            capture_output_switch_rx,
            stream_bitrate_rx,
        ) {
            Ok(()) => {
                helper_log(
                    "capture_pipeline_exit_ok",
                    Some(&format!("elapsed_ms={}", start.elapsed().as_millis())),
                );
                helper_log("helper_exited_ok", Some(helper_role));
                return Ok(());
            }
            Err(e) => {
                let err_detail = format!("{e:#}");
                helper_log(
                    "capture_pipeline_exit_err",
                    Some(&format!(
                        "elapsed_ms={} err={}",
                        start.elapsed().as_millis(),
                        err_detail
                    )),
                );
                helper_log(
                    "helper_exited_err",
                    Some(&format!("role={} err={}", helper_role, err_detail)),
                );
                return Err(e);
            }
        }
    }
    match talos_worker::encode::run_capture_encode_stream_to_pipe(
        &pipe_name,
        &auth_token,
        tuning,
        30,
        talos_worker::encode::parse_display_stream_mode(display_stream_mode.as_deref()),
        stop,
        capture_output_switch_rx,
        stream_bitrate_rx,
    ) {
        Ok(()) => {
            helper_log(
                "capture_pipeline_exit_ok",
                Some(&format!("elapsed_ms={}", start.elapsed().as_millis())),
            );
            helper_log("helper_exited_ok", Some(helper_role));
            Ok(())
        }
        Err(e) => {
            let err_detail = format!("{e:#}");
            helper_log(
                "capture_pipeline_exit_err",
                Some(&format!(
                    "elapsed_ms={} err={}",
                    start.elapsed().as_millis(),
                    err_detail
                )),
            );
            helper_log(
                "helper_exited_err",
                Some(&format!("role={} err={}", helper_role, err_detail)),
            );
            Err(e)
        }
    }
}

#[cfg(target_os = "windows")]
fn main() {
    let outcome = run_helper();
    if let Err(ref e) = outcome {
        eprintln!("{e:#}");
    }
    pause_console_before_exit();
    if outcome.is_err() {
        std::process::exit(1);
    }
}

#[cfg(target_os = "macos")]
fn main() {
    if matches!(
        std::env::args().nth(1).as_deref(),
        Some("check-macos-permissions")
    ) {
        if let Err(err) = macos::run_permission_check_from_args() {
            eprintln!("{err:#}");
            std::process::exit(1);
        }
        return;
    }
    if matches!(
        std::env::args().nth(1).as_deref(),
        Some(
            "capture-macos-h264"
                | "capture-macos-legacy"
                | "capture-macos-atx2"
                | "capture-macos-screenshot"
        )
    ) {
        if let Err(err) = macos::run_from_args() {
            eprintln!("{err:#}");
            std::process::exit(1);
        }
        return;
    }
    eprintln!("Talos Worker helper on macOS expects capture-macos-h264, capture-macos-legacy, capture-macos-atx2, capture-macos-screenshot, or check-macos-permissions");
    std::process::exit(1);
}

#[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
fn main() {
    eprintln!("Talos Worker helper is only supported on Windows");
    std::process::exit(1);
}
