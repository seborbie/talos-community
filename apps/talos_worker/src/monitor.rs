#[cfg(target_os = "windows")]
pub(crate) fn get_primary_monitor_hz() -> Option<u32> {
    use std::ptr::null_mut;
    use winapi::um::wingdi::DEVMODEW;
    use winapi::um::winuser::{EnumDisplaySettingsW, ENUM_CURRENT_SETTINGS};

    let mut devmode: DEVMODEW = unsafe { std::mem::zeroed() };
    devmode.dmSize = std::mem::size_of::<DEVMODEW>() as u16;
    let ok = unsafe { EnumDisplaySettingsW(null_mut(), ENUM_CURRENT_SETTINGS, &mut devmode) != 0 };
    if ok && devmode.dmDisplayFrequency != 0 {
        Some(devmode.dmDisplayFrequency)
    } else {
        None
    }
}

/// GDI device name for the primary monitor (e.g. `\\.\DISPLAY1`), matching
/// [`DXGI_OUTPUT_DESC::DeviceName`] for that output when available.
#[cfg(target_os = "windows")]
pub(crate) fn primary_gdi_display_device_name() -> Option<String> {
    use std::mem;
    use winapi::shared::windef::POINT;
    use winapi::um::winuser::{
        GetMonitorInfoW, MonitorFromPoint, MONITORINFO, MONITORINFOEXW, MONITOR_DEFAULTTOPRIMARY,
    };

    let pt = POINT { x: 0, y: 0 };
    let hmon = unsafe { MonitorFromPoint(pt, MONITOR_DEFAULTTOPRIMARY) };
    if hmon.is_null() {
        return None;
    }
    let mut info: MONITORINFOEXW = unsafe { mem::zeroed() };
    info.cbSize = mem::size_of::<MONITORINFOEXW>() as u32;
    let ok = unsafe { GetMonitorInfoW(hmon, &mut info as *mut MONITORINFOEXW as *mut MONITORINFO) };
    if ok == 0 {
        return None;
    }
    let end = info
        .szDevice
        .iter()
        .position(|&c| c == 0)
        .unwrap_or(info.szDevice.len());
    let s = String::from_utf16_lossy(&info.szDevice[..end]);
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

#[cfg(target_os = "windows")]
#[derive(Clone, Debug)]
pub(crate) struct GdiMonitorInfo {
    pub device_name: String,
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
    pub is_primary: bool,
}

#[cfg(target_os = "windows")]
fn gdi_monitor_sort_key(device_name: &str) -> (u32, String) {
    let upper = device_name.to_ascii_uppercase();
    let display_index = upper
        .strip_prefix(r"\\.\DISPLAY")
        .and_then(|suffix| suffix.parse::<u32>().ok())
        .unwrap_or(u32::MAX);
    (display_index, upper)
}

#[cfg(target_os = "windows")]
pub(crate) fn enumerate_gdi_monitors() -> Vec<GdiMonitorInfo> {
    use std::{mem, ptr};
    use winapi::shared::{
        minwindef::{BOOL, LPARAM},
        windef::{HDC, HMONITOR, LPRECT},
    };
    use winapi::um::winuser::{
        EnumDisplayMonitors, GetMonitorInfoW, MONITORINFO, MONITORINFOEXW, MONITORINFOF_PRIMARY,
    };

    unsafe extern "system" fn collect_monitor(
        hmonitor: HMONITOR,
        _hdc: HDC,
        _clip_rect: LPRECT,
        lparam: LPARAM,
    ) -> BOOL {
        let monitors = &mut *(lparam as *mut Vec<GdiMonitorInfo>);
        let mut info: MONITORINFOEXW = mem::zeroed();
        info.cbSize = mem::size_of::<MONITORINFOEXW>() as u32;
        if GetMonitorInfoW(
            hmonitor,
            &mut info as *mut MONITORINFOEXW as *mut MONITORINFO,
        ) == 0
        {
            return 1;
        }

        let end = info
            .szDevice
            .iter()
            .position(|&c| c == 0)
            .unwrap_or(info.szDevice.len());
        let device_name = String::from_utf16_lossy(&info.szDevice[..end]);
        let rect = info.rcMonitor;
        monitors.push(GdiMonitorInfo {
            device_name,
            left: rect.left,
            top: rect.top,
            right: rect.right,
            bottom: rect.bottom,
            is_primary: (info.dwFlags & MONITORINFOF_PRIMARY) != 0,
        });
        1
    }

    let mut monitors: Vec<GdiMonitorInfo> = Vec::new();
    unsafe {
        EnumDisplayMonitors(
            ptr::null_mut(),
            ptr::null(),
            Some(collect_monitor),
            &mut monitors as *mut Vec<GdiMonitorInfo> as LPARAM,
        );
    }
    monitors.sort_by(|a, b| {
        gdi_monitor_sort_key(&a.device_name).cmp(&gdi_monitor_sort_key(&b.device_name))
    });
    monitors
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn get_primary_monitor_hz() -> Option<u32> {
    None
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn primary_gdi_display_device_name() -> Option<String> {
    None
}

#[cfg(all(test, target_os = "windows"))]
mod tests {
    use super::gdi_monitor_sort_key;

    #[test]
    fn gdi_monitor_sort_key_orders_display_numbers_naturally() {
        assert!(gdi_monitor_sort_key(r"\\.\DISPLAY2") < gdi_monitor_sort_key(r"\\.\DISPLAY10"));
    }
}
