use crate::windows_utils::wmi::WmiHelper;
use anyhow::Result;
use chrono::{DateTime, Duration, Local, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct NativeEventRecord {
    pub time: DateTime<Utc>,
    pub source: String,
    pub event_id: u32,
    pub level: String,
    pub message: String,
    pub computer: String,
    pub user: Option<String>,
}

/// Map Win32_NTLogEvent Type (numeric or string) to level string.
/// Type: 1=Error, 2=Warning, 3=Information, 4=Success Audit, 5=Failure Audit.
fn type_to_level(value: &serde_json::Value) -> String {
    if let Some(n) = value.as_u64().or_else(|| value.as_i64().map(|i| i as u64)) {
        return match n {
            1 => "Error".to_string(),
            2 => "Warning".to_string(),
            3 => "Information".to_string(),
            4 => "Success Audit".to_string(),
            5 => "Failure Audit".to_string(),
            _ => "Unknown".to_string(),
        };
    }
    value
        .as_str()
        .map(|s| s.to_string())
        .unwrap_or_else(|| "Unknown".to_string())
}

/// Format a UTC time for WMI TimeGenerated comparison. WMI stores in local time with offset
/// (e.g. 20260221130723.000000-480). Convert to local and format so the filter matches.
fn format_wmi_datetime(dt: DateTime<Utc>) -> String {
    let local: DateTime<Local> = dt.with_timezone(&Local);
    let offset_min = local.offset().local_minus_utc();
    let sign = if offset_min >= 0 { '+' } else { '-' };
    format!(
        "{}.000000{}{:03}",
        local.format("%Y%m%d%H%M%S"),
        sign,
        offset_min.abs()
    )
}

pub async fn query_events(
    logfile: &str,
    start_time: DateTime<Utc>,
    max_results: usize,
    where_clause: Option<&str>,
) -> Result<Vec<NativeEventRecord>> {
    let start = format_wmi_datetime(start_time);
    let mut wql = format!(
        "SELECT Logfile, EventCode, Type, SourceName, Message, TimeGenerated, ComputerName, User \
         FROM Win32_NTLogEvent WHERE Logfile='{}' AND TimeGenerated >= '{}'",
        logfile, start
    );
    if let Some(extra) = where_clause {
        wql.push_str(" AND (");
        wql.push_str(extra);
        wql.push(')');
    }
    wql.push_str(" ORDER BY TimeGenerated DESC");

    let rows = WmiHelper::query_nt_events(&wql).await.unwrap_or_default();
    let mut results = Vec::new();
    for row in rows.into_iter().take(max_results) {
        let time = row
            .get("TimeGenerated")
            .and_then(|v| v.as_str())
            .and_then(WmiHelper::parse_wmi_datetime_str)
            .unwrap_or_else(Utc::now);
        let source = row
            .get("SourceName")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .unwrap_or_default();
        let event_id = row.get("EventCode").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
        let level = row
            .get("Type")
            .map(type_to_level)
            .unwrap_or_else(|| "Unknown".to_string());
        let message = row
            .get("Message")
            .and_then(|v| v.as_str())
            .map(|s| s.lines().next().unwrap_or(s).to_string())
            .unwrap_or_default();
        let computer = row
            .get("ComputerName")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .unwrap_or_default();
        let user = row
            .get("User")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .filter(|s| !s.is_empty());

        results.push(NativeEventRecord {
            time,
            source,
            event_id,
            level,
            message,
            computer,
            user,
        });
    }
    Ok(results)
}

pub fn now_minus_hours(hours: i64) -> DateTime<Utc> {
    Utc::now() - Duration::hours(hours)
}

pub fn now_minus_days(days: i64) -> DateTime<Utc> {
    Utc::now() - Duration::days(days)
}
