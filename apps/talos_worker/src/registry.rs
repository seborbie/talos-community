use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::ptr;

use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use base64::Engine as _;
use talos_protocol::{
    RegistryErrorCode, RegistryHive, RegistryRequest, RegistryResponse, RegistryResponseEnvelope,
    RegistryValueData, RegistryValueEntry, REGISTRY_META_MESSAGE_TYPE,
};
use winapi::shared::minwindef::{DWORD, FILETIME, HKEY};
use winapi::shared::winerror::{
    ERROR_ACCESS_DENIED, ERROR_FILE_NOT_FOUND, ERROR_MORE_DATA, ERROR_NO_MORE_ITEMS,
    ERROR_PATH_NOT_FOUND, ERROR_SUCCESS,
};
use winapi::um::winnt::{
    KEY_READ, KEY_SET_VALUE, KEY_WRITE, REG_BINARY, REG_DWORD, REG_EXPAND_SZ, REG_MULTI_SZ,
    REG_QWORD, REG_SZ,
};
use winapi::um::winreg::{
    RegCloseKey, RegCreateKeyExW, RegDeleteKeyW, RegDeleteTreeW, RegDeleteValueW, RegEnumKeyExW,
    RegEnumValueW, RegOpenKeyExW, RegQueryInfoKeyW, RegQueryValueExW, RegSetValueExW,
    HKEY_CLASSES_ROOT, HKEY_CURRENT_CONFIG, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, HKEY_USERS,
};

const LIST_VALUE_BINARY_MAX_BYTES: usize = 16 * 1024;
const READ_VALUE_MAX_BYTES: usize = 256 * 1024;
const WRITE_VALUE_MAX_BYTES: usize = 256 * 1024;

#[derive(Debug, Clone)]
pub struct RegistryOpError {
    pub code: RegistryErrorCode,
    pub message: String,
}

impl RegistryOpError {
    fn new(code: RegistryErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for RegistryOpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} ({:?})", self.message, self.code)
    }
}

impl std::error::Error for RegistryOpError {}

fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn normalize_reg_path(path: &str) -> String {
    let trimmed = path.trim();
    let trimmed = trimmed.trim_matches(['\\', '/']);
    trimmed.replace('/', "\\")
}

fn hive_root(hive: RegistryHive) -> HKEY {
    match hive {
        RegistryHive::HKLM => HKEY_LOCAL_MACHINE,
        RegistryHive::HKCU => HKEY_CURRENT_USER,
        RegistryHive::HKCR => HKEY_CLASSES_ROOT,
        RegistryHive::HKU => HKEY_USERS,
        RegistryHive::HKCC => HKEY_CURRENT_CONFIG,
    }
}

struct RegKey {
    hkey: HKEY,
    close: bool,
}

impl Drop for RegKey {
    fn drop(&mut self) {
        if self.close {
            unsafe {
                let _ = RegCloseKey(self.hkey);
            }
        }
    }
}

fn map_winreg_status(status: u32, context: &str) -> RegistryOpError {
    let code = match status {
        ERROR_ACCESS_DENIED => RegistryErrorCode::PermissionDenied,
        ERROR_FILE_NOT_FOUND | ERROR_PATH_NOT_FOUND => RegistryErrorCode::NotFound,
        ERROR_MORE_DATA => RegistryErrorCode::PayloadTooLarge,
        _ => RegistryErrorCode::Internal,
    };
    RegistryOpError::new(code, format!("{context} failed (winerr {status})"))
}

fn open_key(hive: RegistryHive, path: &str, access: u32) -> Result<RegKey, RegistryOpError> {
    let path = normalize_reg_path(path);
    if path.is_empty() {
        return Ok(RegKey {
            hkey: hive_root(hive),
            close: false,
        });
    }
    let root = hive_root(hive);
    let wide = to_wide(&path);
    let mut out: HKEY = ptr::null_mut();
    let status = unsafe { RegOpenKeyExW(root, wide.as_ptr(), 0, access, &mut out) } as u32;
    if status != ERROR_SUCCESS {
        return Err(map_winreg_status(status, "RegOpenKeyExW"));
    }
    Ok(RegKey {
        hkey: out,
        close: true,
    })
}

fn registry_type_name(raw_type: u32) -> String {
    match raw_type {
        0 => "REG_NONE",
        1 => "REG_SZ",
        2 => "REG_EXPAND_SZ",
        3 => "REG_BINARY",
        4 => "REG_DWORD",
        5 => "REG_DWORD_BIG_ENDIAN",
        6 => "REG_LINK",
        7 => "REG_MULTI_SZ",
        8 => "REG_RESOURCE_LIST",
        9 => "REG_FULL_RESOURCE_DESCRIPTOR",
        10 => "REG_RESOURCE_REQUIREMENTS_LIST",
        11 => "REG_QWORD",
        _ => return format!("REG_0x{raw_type:08X}"),
    }
    .to_string()
}

fn parse_registry_u64(input: &str) -> Result<u64, RegistryOpError> {
    let s = input.trim();
    if s.is_empty() {
        return Err(RegistryOpError::new(
            RegistryErrorCode::InvalidType,
            "invalid qword value (empty)",
        ));
    }
    let (radix, digits) = if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        (16, hex)
    } else {
        (10, s)
    };
    u64::from_str_radix(digits.trim(), radix)
        .map_err(|_| RegistryOpError::new(RegistryErrorCode::InvalidType, "invalid qword value"))
}

fn bytes_to_u16_vec_le(bytes: &[u8]) -> Vec<u16> {
    bytes
        .chunks_exact(2)
        .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
        .collect()
}

fn decode_reg_sz(bytes: &[u8]) -> String {
    let mut wide = bytes_to_u16_vec_le(bytes);
    while wide.last().copied() == Some(0) {
        wide.pop();
    }
    OsString::from_wide(&wide).to_string_lossy().to_string()
}

fn decode_reg_multi_sz(bytes: &[u8]) -> Vec<String> {
    let wide = bytes_to_u16_vec_le(bytes);
    let mut out: Vec<String> = Vec::new();
    let mut current: Vec<u16> = Vec::new();
    for ch in wide {
        if ch == 0 {
            if current.is_empty() {
                break;
            }
            out.push(OsString::from_wide(&current).to_string_lossy().to_string());
            current.clear();
            continue;
        }
        current.push(ch);
    }
    out
}

fn parse_value_data(raw_type: u32, bytes: &[u8], list_mode: bool) -> RegistryValueData {
    match raw_type {
        REG_SZ | REG_EXPAND_SZ => RegistryValueData::Sz {
            data: decode_reg_sz(bytes),
        },
        REG_DWORD if bytes.len() >= 4 => RegistryValueData::Dword {
            data: u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]),
        },
        REG_QWORD if bytes.len() >= 8 => RegistryValueData::Qword {
            data: u64::from_le_bytes([
                bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
            ])
            .to_string(),
        },
        REG_MULTI_SZ => RegistryValueData::MultiSz {
            data: decode_reg_multi_sz(bytes),
        },
        REG_BINARY => {
            let slice = if list_mode && bytes.len() > LIST_VALUE_BINARY_MAX_BYTES {
                &bytes[..LIST_VALUE_BINARY_MAX_BYTES]
            } else {
                bytes
            };
            RegistryValueData::Binary {
                data_b64: BASE64_STANDARD.encode(slice),
            }
        }
        _ => RegistryValueData::Unknown {
            raw_type,
            data_b64: BASE64_STANDARD.encode(bytes),
        },
    }
}

pub fn handle_request(request: RegistryRequest) -> RegistryResponseEnvelope {
    let request_id = registry_request_id(&request).to_string();
    let session_id = registry_session_id(&request).to_string();

    let response = match execute_request(request) {
        Ok(resp) => resp,
        Err(err) => RegistryResponse::Error {
            code: err.code,
            message: err.message,
        },
    };
    RegistryResponseEnvelope {
        message_type: REGISTRY_META_MESSAGE_TYPE.to_string(),
        request_id,
        session_id,
        response,
    }
}

fn registry_request_id(request: &RegistryRequest) -> &str {
    match request {
        RegistryRequest::ListKeys { request_id, .. }
        | RegistryRequest::ListValues { request_id, .. }
        | RegistryRequest::GetValue { request_id, .. }
        | RegistryRequest::SetValue { request_id, .. }
        | RegistryRequest::CreateKey { request_id, .. }
        | RegistryRequest::DeleteKey { request_id, .. }
        | RegistryRequest::DeleteValue { request_id, .. }
        | RegistryRequest::Cancel { request_id, .. } => request_id,
    }
}

fn registry_session_id(request: &RegistryRequest) -> &str {
    match request {
        RegistryRequest::ListKeys { session_id, .. }
        | RegistryRequest::ListValues { session_id, .. }
        | RegistryRequest::GetValue { session_id, .. }
        | RegistryRequest::SetValue { session_id, .. }
        | RegistryRequest::CreateKey { session_id, .. }
        | RegistryRequest::DeleteKey { session_id, .. }
        | RegistryRequest::DeleteValue { session_id, .. }
        | RegistryRequest::Cancel { session_id, .. } => session_id,
    }
}

fn normalize_registry_page(offset: u32, limit: u32) -> (usize, usize) {
    let offset = offset as usize;
    let limit = if limit == 0 {
        256
    } else {
        limit.min(1024) as usize
    };
    (offset, limit)
}

fn paginate_items<T: Clone>(items: Vec<T>, offset: u32, limit: u32) -> (Vec<T>, Option<u32>, u32) {
    let total_count = items.len() as u32;
    let (offset, limit) = normalize_registry_page(offset, limit);
    if offset >= items.len() {
        return (Vec::new(), None, total_count);
    }
    let end = offset.saturating_add(limit).min(items.len());
    let next_offset = if end < items.len() {
        Some(end as u32)
    } else {
        None
    };
    (items[offset..end].to_vec(), next_offset, total_count)
}

fn execute_request(request: RegistryRequest) -> Result<RegistryResponse, RegistryOpError> {
    match request {
        RegistryRequest::ListKeys {
            hive,
            path,
            offset,
            limit,
            ..
        } => {
            let keys = list_keys(hive, &path)?;
            let (keys, next_offset, total_count) = paginate_items(keys, offset, limit);
            Ok(RegistryResponse::ListKeys {
                keys,
                next_offset,
                total_count: Some(total_count),
            })
        }
        RegistryRequest::ListValues {
            hive,
            path,
            offset,
            limit,
            ..
        } => {
            let values = list_values(hive, &path)?;
            let (values, next_offset, total_count) = paginate_items(values, offset, limit);
            Ok(RegistryResponse::ListValues {
                values,
                next_offset,
                total_count: Some(total_count),
            })
        }
        RegistryRequest::GetValue {
            hive, path, name, ..
        } => {
            let value = get_value(hive, &path, &name)?;
            Ok(RegistryResponse::GetValue { value })
        }
        RegistryRequest::SetValue {
            hive,
            path,
            name,
            data,
            ..
        } => {
            set_value(hive, &path, &name, data)?;
            Ok(RegistryResponse::Ok {})
        }
        RegistryRequest::CreateKey { hive, path, .. } => {
            create_key(hive, &path)?;
            Ok(RegistryResponse::Ok {})
        }
        RegistryRequest::DeleteKey {
            hive,
            path,
            recursive,
            ..
        } => {
            delete_key(hive, &path, recursive)?;
            Ok(RegistryResponse::Ok {})
        }
        RegistryRequest::DeleteValue {
            hive, path, name, ..
        } => {
            delete_value(hive, &path, &name)?;
            Ok(RegistryResponse::Ok {})
        }
        RegistryRequest::Cancel { .. } => Ok(RegistryResponse::Ok {}),
    }
}

fn list_keys(hive: RegistryHive, path: &str) -> Result<Vec<String>, RegistryOpError> {
    let key = open_key(hive, path, KEY_READ)?;
    let mut subkeys: DWORD = 0;
    let mut max_subkey_len: DWORD = 0;
    let status = unsafe {
        RegQueryInfoKeyW(
            key.hkey,
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            &mut subkeys,
            &mut max_subkey_len,
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
        )
    } as u32;
    if status != ERROR_SUCCESS {
        return Err(map_winreg_status(status, "RegQueryInfoKeyW"));
    }

    let mut buf = vec![0u16; (max_subkey_len as usize).saturating_add(1).max(256)];
    let mut out: Vec<String> = Vec::with_capacity(subkeys as usize);
    let mut index: DWORD = 0;
    loop {
        let mut name_len: DWORD = buf.len() as DWORD;
        let mut last_write: FILETIME = unsafe { std::mem::zeroed() };
        let status = unsafe {
            RegEnumKeyExW(
                key.hkey,
                index,
                buf.as_mut_ptr(),
                &mut name_len,
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null_mut(),
                &mut last_write,
            )
        } as u32;
        if status == ERROR_NO_MORE_ITEMS {
            break;
        }
        if status == ERROR_MORE_DATA {
            // Grow buffer and retry same index.
            let next = buf.len().saturating_mul(2).max(512);
            buf.resize(next, 0);
            continue;
        }
        if status != ERROR_SUCCESS {
            return Err(map_winreg_status(status, "RegEnumKeyExW"));
        }
        let name = OsString::from_wide(&buf[..name_len as usize])
            .to_string_lossy()
            .to_string();
        out.push(name);
        index = index.saturating_add(1);
    }
    out.sort_by_key(|a| a.to_lowercase());
    Ok(out)
}

fn list_values(hive: RegistryHive, path: &str) -> Result<Vec<RegistryValueEntry>, RegistryOpError> {
    let key = open_key(hive, path, KEY_READ)?;
    let mut values_count: DWORD = 0;
    let mut max_value_name_len: DWORD = 0;
    let mut max_value_data_len: DWORD = 0;
    let status = unsafe {
        RegQueryInfoKeyW(
            key.hkey,
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            &mut values_count,
            &mut max_value_name_len,
            &mut max_value_data_len,
            ptr::null_mut(),
            ptr::null_mut(),
        )
    } as u32;
    if status != ERROR_SUCCESS {
        return Err(map_winreg_status(status, "RegQueryInfoKeyW"));
    }

    let mut name_buf = vec![0u16; (max_value_name_len as usize).saturating_add(1).max(256)];
    let mut data_buf = vec![0u8; (max_value_data_len as usize).min(4096).max(256)];
    let mut out: Vec<RegistryValueEntry> = Vec::with_capacity(values_count as usize);

    let mut index: DWORD = 0;
    loop {
        let mut name_len: DWORD = name_buf.len() as DWORD;
        let mut raw_type: DWORD = 0;
        let mut data_len: DWORD = data_buf.len() as DWORD;
        let status = unsafe {
            RegEnumValueW(
                key.hkey,
                index,
                name_buf.as_mut_ptr(),
                &mut name_len,
                ptr::null_mut(),
                &mut raw_type,
                data_buf.as_mut_ptr(),
                &mut data_len,
            )
        } as u32;
        if status == ERROR_NO_MORE_ITEMS {
            break;
        }
        if status == ERROR_MORE_DATA {
            let needed = data_len as usize;
            if needed > READ_VALUE_MAX_BYTES {
                return Err(RegistryOpError::new(
                    RegistryErrorCode::PayloadTooLarge,
                    "registry value data too large to enumerate",
                ));
            }
            data_buf.resize(needed.max(256), 0u8);
            continue;
        }
        if status != ERROR_SUCCESS {
            return Err(map_winreg_status(status, "RegEnumValueW"));
        }
        let name = OsString::from_wide(&name_buf[..name_len as usize])
            .to_string_lossy()
            .to_string();
        let raw_type_u32 = raw_type as u32;
        let value_type = registry_type_name(raw_type_u32);
        let bytes = &data_buf[..data_len as usize];
        let data = parse_value_data(raw_type_u32, bytes, true);
        out.push(RegistryValueEntry {
            name,
            value_type,
            raw_type: raw_type_u32,
            data,
        });
        index = index.saturating_add(1);
    }
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    Ok(out)
}

fn get_value(
    hive: RegistryHive,
    path: &str,
    name: &str,
) -> Result<Option<RegistryValueEntry>, RegistryOpError> {
    let key = open_key(hive, path, KEY_READ)?;
    let wide_name = to_wide(name);
    let mut raw_type: DWORD = 0;
    let mut data_len: DWORD = 0;
    let status = unsafe {
        RegQueryValueExW(
            key.hkey,
            wide_name.as_ptr(),
            ptr::null_mut(),
            &mut raw_type,
            ptr::null_mut(),
            &mut data_len,
        )
    } as u32;
    if status == ERROR_FILE_NOT_FOUND {
        return Ok(None);
    }
    if status != ERROR_SUCCESS {
        return Err(map_winreg_status(status, "RegQueryValueExW(size)"));
    }
    if data_len as usize > READ_VALUE_MAX_BYTES {
        return Err(RegistryOpError::new(
            RegistryErrorCode::PayloadTooLarge,
            format!("registry value too large ({data_len} bytes)"),
        ));
    }
    let mut data = vec![0u8; data_len as usize];
    let status = unsafe {
        RegQueryValueExW(
            key.hkey,
            wide_name.as_ptr(),
            ptr::null_mut(),
            &mut raw_type,
            data.as_mut_ptr(),
            &mut data_len,
        )
    } as u32;
    if status != ERROR_SUCCESS {
        return Err(map_winreg_status(status, "RegQueryValueExW(data)"));
    }
    data.truncate(data_len as usize);
    let raw_type_u32 = raw_type as u32;
    let value_type = registry_type_name(raw_type_u32);
    let parsed = parse_value_data(raw_type_u32, &data, false);
    Ok(Some(RegistryValueEntry {
        name: name.to_string(),
        value_type,
        raw_type: raw_type_u32,
        data: parsed,
    }))
}

fn set_value(
    hive: RegistryHive,
    path: &str,
    name: &str,
    data: RegistryValueData,
) -> Result<(), RegistryOpError> {
    let key = open_key(hive, path, KEY_WRITE)?;
    let wide_name = to_wide(name);
    let (raw_type, bytes): (DWORD, Vec<u8>) = match data {
        RegistryValueData::Sz { data } => {
            let wide = to_wide(&data);
            let mut bytes: Vec<u8> = Vec::with_capacity(wide.len() * 2);
            for ch in wide {
                bytes.extend_from_slice(&ch.to_le_bytes());
            }
            (REG_SZ, bytes)
        }
        RegistryValueData::Dword { data } => (REG_DWORD, data.to_le_bytes().to_vec()),
        RegistryValueData::Qword { data } => {
            (REG_QWORD, parse_registry_u64(&data)?.to_le_bytes().to_vec())
        }
        RegistryValueData::MultiSz { data } => {
            let mut wide: Vec<u16> = Vec::new();
            for (idx, item) in data.iter().enumerate() {
                if idx > 0 {
                    // Terminator between strings.
                    wide.push(0);
                }
                wide.extend(item.encode_utf16());
            }
            wide.push(0);
            wide.push(0);
            let mut bytes: Vec<u8> = Vec::with_capacity(wide.len() * 2);
            for ch in wide {
                bytes.extend_from_slice(&ch.to_le_bytes());
            }
            (REG_MULTI_SZ, bytes)
        }
        RegistryValueData::Binary { data_b64 } => {
            let decoded = BASE64_STANDARD.decode(data_b64.as_bytes()).map_err(|_| {
                RegistryOpError::new(RegistryErrorCode::InvalidType, "invalid base64 binary")
            })?;
            (REG_BINARY, decoded)
        }
        RegistryValueData::Unknown { .. } => {
            return Err(RegistryOpError::new(
                RegistryErrorCode::InvalidType,
                "writing unknown registry value types is not supported",
            ));
        }
    };
    if bytes.len() > WRITE_VALUE_MAX_BYTES {
        return Err(RegistryOpError::new(
            RegistryErrorCode::PayloadTooLarge,
            "registry write payload too large",
        ));
    }
    let status = unsafe {
        RegSetValueExW(
            key.hkey,
            wide_name.as_ptr(),
            0,
            raw_type,
            bytes.as_ptr(),
            bytes.len() as DWORD,
        )
    } as u32;
    if status != ERROR_SUCCESS {
        return Err(map_winreg_status(status, "RegSetValueExW"));
    }
    Ok(())
}

fn create_key(hive: RegistryHive, path: &str) -> Result<(), RegistryOpError> {
    let path = normalize_reg_path(path);
    if path.is_empty() {
        return Err(RegistryOpError::new(
            RegistryErrorCode::InvalidPath,
            "cannot create hive root",
        ));
    }
    let root = hive_root(hive);
    let wide = to_wide(&path);
    let mut out: HKEY = ptr::null_mut();
    let mut disposition: DWORD = 0;
    let status = unsafe {
        RegCreateKeyExW(
            root,
            wide.as_ptr(),
            0,
            ptr::null_mut(),
            0,
            KEY_WRITE,
            ptr::null_mut(),
            &mut out,
            &mut disposition,
        )
    } as u32;
    if status != ERROR_SUCCESS {
        return Err(map_winreg_status(status, "RegCreateKeyExW"));
    }
    unsafe {
        let _ = RegCloseKey(out);
    }
    Ok(())
}

fn delete_key(hive: RegistryHive, path: &str, recursive: bool) -> Result<(), RegistryOpError> {
    let path = normalize_reg_path(path);
    if path.is_empty() {
        return Err(RegistryOpError::new(
            RegistryErrorCode::InvalidPath,
            "refusing to delete hive root",
        ));
    }
    let root = hive_root(hive);
    let wide = to_wide(&path);
    let status = unsafe {
        if recursive {
            RegDeleteTreeW(root, wide.as_ptr())
        } else {
            RegDeleteKeyW(root, wide.as_ptr())
        }
    } as u32;
    if status != ERROR_SUCCESS {
        return Err(map_winreg_status(
            status,
            if recursive {
                "RegDeleteTreeW"
            } else {
                "RegDeleteKeyW"
            },
        ));
    }
    Ok(())
}

fn delete_value(hive: RegistryHive, path: &str, name: &str) -> Result<(), RegistryOpError> {
    let key = open_key(hive, path, KEY_SET_VALUE)?;
    let wide_name = to_wide(name);
    let status = unsafe { RegDeleteValueW(key.hkey, wide_name.as_ptr()) } as u32;
    if status != ERROR_SUCCESS {
        return Err(map_winreg_status(status, "RegDeleteValueW"));
    }
    Ok(())
}
