use crate::windows_utils::wmi::WmiHelper;
#[cfg(windows)]
use anyhow::Context;
use anyhow::Result;
use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
#[cfg(windows)]
use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};
use tokio::sync::Mutex as AsyncMutex;
#[cfg(windows)]
use tracing::warn;
use tracing::{debug, trace};
#[cfg(windows)]
use windows::{
    core::{BSTR, HSTRING},
    Win32::Foundation::{RPC_E_CHANGED_MODE, VARIANT_BOOL},
    Win32::System::{
        Com::{
            CLSIDFromProgID, CoCreateInstance, CoInitializeEx, CoUninitialize,
            CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
        },
        UpdateAgent::{IUpdateSearcher, IUpdateSession, ServerSelection},
    },
};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct NativePendingUpdate {
    pub title: String,
    pub kb: Option<String>,
    pub is_mandatory: bool,
    pub size: Option<u64>,
    pub reboot_required: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct NativeUpdateHistory {
    pub date: DateTime<Utc>,
    pub title: String,
    pub operation: String,
    pub result: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct NativeWindowsUpdateStatus {
    pub pending_update_count: u32,
    pub pending_updates: Vec<NativePendingUpdate>,
    pub optional_updates: Vec<String>,
    pub driver_updates: Vec<String>,
    pub history: Vec<NativeUpdateHistory>,
    pub last_search_time: Option<DateTime<Utc>>,
}

const WU_STATUS_CACHE_TTL: Duration = Duration::from_secs(60);
#[cfg(windows)]
const WU_COM_TIMEOUT: Duration = Duration::from_secs(120);
#[cfg(windows)]
const WU_UPGRADES_CATEGORY_ID: &str = "3689BDC8-B205-4AF4-8D4A-A63924C5E9D5";
static WU_STATUS_CACHE: OnceLock<Mutex<Option<(Instant, NativeWindowsUpdateStatus)>>> =
    OnceLock::new();
static WU_STATUS_FETCH_LOCK: OnceLock<AsyncMutex<()>> = OnceLock::new();

/// Collect Windows Update details without PowerShell.
///
/// Uses Windows Update Agent COM as the native source for pending updates and
/// WMI hotfix inventory as a fallback source for installed history.
pub async fn get_windows_update_status() -> Result<NativeWindowsUpdateStatus> {
    if let Some(cached) = get_cached_status() {
        trace!(
            pending_update_count = cached.pending_update_count,
            pending_updates = cached.pending_updates.len(),
            history = cached.history.len(),
            "Windows Update status cache hit"
        );
        return Ok(cached);
    }

    // Single-flight guard: avoid multiple concurrent COM queries on startup.
    // Software + Updates collectors can ask at the same time.
    let fetch_lock = WU_STATUS_FETCH_LOCK.get_or_init(|| AsyncMutex::new(()));
    trace!("Windows Update status fetch lock waiting");
    let _guard = fetch_lock.lock().await;
    trace!("Windows Update status fetch lock acquired");

    // Re-check cache after acquiring lock in case another waiter already fetched it.
    if let Some(cached) = get_cached_status() {
        trace!(
            pending_update_count = cached.pending_update_count,
            pending_updates = cached.pending_updates.len(),
            history = cached.history.len(),
            "Windows Update status cache hit after lock"
        );
        return Ok(cached);
    }

    #[cfg(windows)]
    let com_start = Instant::now();
    #[cfg(windows)]
    debug!(
        timeout_secs = WU_COM_TIMEOUT.as_secs(),
        "Windows Update COM collection task starting"
    );
    #[cfg(windows)]
    match tokio::time::timeout(
        WU_COM_TIMEOUT,
        tokio::task::spawn_blocking(collect_windows_update_status_com),
    )
    .await
    {
        Ok(joined) => match joined {
            Ok(Ok(status)) => {
                debug!(
                    elapsed_ms = com_start.elapsed().as_millis(),
                    pending_update_count = status.pending_update_count,
                    pending_updates = status.pending_updates.len(),
                    optional_updates = status.optional_updates.len(),
                    driver_updates = status.driver_updates.len(),
                    history = status.history.len(),
                    "Windows Update COM collection succeeded"
                );
                set_cached_status(status.clone());
                return Ok(status);
            }
            Ok(Err(e)) => warn!(
                elapsed_ms = com_start.elapsed().as_millis(),
                error = %e,
                "Windows Update COM collection failed, using fallback"
            ),
            Err(e) => warn!(
                elapsed_ms = com_start.elapsed().as_millis(),
                error = %e,
                "Windows Update COM task join failed, using fallback"
            ),
        },
        Err(_) => warn!(
            timeout_secs = WU_COM_TIMEOUT.as_secs(),
            elapsed_ms = com_start.elapsed().as_millis(),
            "Windows Update COM collection exceeded timeout, using fallback"
        ),
    }

    // Fallback path if COM is unavailable/fails.
    debug!("Windows Update WMI fallback collection starting");
    let fallback_start = Instant::now();
    let fallback = collect_windows_update_status_wmi_fallback().await;
    debug!(
        elapsed_ms = fallback_start.elapsed().as_millis(),
        history = fallback.history.len(),
        pending_update_count = fallback.pending_update_count,
        "Windows Update WMI fallback collection completed"
    );
    set_cached_status(fallback.clone());
    Ok(fallback)
}

async fn collect_windows_update_status_wmi_fallback() -> NativeWindowsUpdateStatus {
    let mut status = NativeWindowsUpdateStatus::default();

    trace!("Windows Update WMI fallback installed updates query starting");
    let installed = WmiHelper::get_installed_updates().await.unwrap_or_default();
    trace!(
        installed_updates = installed.len(),
        "Windows Update WMI fallback installed updates query completed"
    );
    for entry in installed {
        let title = entry
            .get("Description")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .filter(|s| !s.is_empty())
            .or_else(|| {
                entry
                    .get("HotFixID")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
            })
            .unwrap_or_else(|| "Windows Update".to_string());

        let install_date = entry
            .get("InstalledOn")
            .and_then(|v| v.as_str())
            .and_then(parse_us_short_date)
            .unwrap_or_else(Utc::now);

        status.history.push(NativeUpdateHistory {
            date: install_date,
            title,
            operation: "Installation".to_string(),
            result: "Succeeded".to_string(),
        });
    }

    status.history.sort_by(|a, b| b.date.cmp(&a.date));
    status.last_search_time = status.history.first().map(|h| h.date);
    status
}

fn parse_us_short_date(value: &str) -> Option<DateTime<Utc>> {
    let date = NaiveDate::parse_from_str(value, "%m/%d/%Y").ok()?;
    let dt = date.and_hms_opt(0, 0, 0)?;
    Some(DateTime::from_naive_utc_and_offset(dt, Utc))
}

#[cfg(windows)]
fn collect_windows_update_status_com() -> Result<NativeWindowsUpdateStatus> {
    struct ComApartmentGuard {
        should_uninitialize: bool,
    }

    impl Drop for ComApartmentGuard {
        fn drop(&mut self) {
            if self.should_uninitialize {
                trace!("WU COM CoUninitialize starting");
                unsafe { CoUninitialize() };
                trace!("WU COM CoUninitialize completed");
            }
        }
    }

    unsafe {
        let started = Instant::now();
        debug!("Windows Update COM collection entered blocking task");

        let step_start = Instant::now();
        trace!("WU COM CoInitializeEx starting");
        let hr = CoInitializeEx(None, COINIT_MULTITHREADED);
        trace!(
            elapsed_ms = step_start.elapsed().as_millis(),
            result = ?hr,
            "WU COM CoInitializeEx completed"
        );
        let guard = if hr.is_ok() {
            trace!("WU COM apartment initialized by collector");
            ComApartmentGuard {
                should_uninitialize: true,
            }
        } else if hr != RPC_E_CHANGED_MODE {
            return Err(anyhow::anyhow!("CoInitializeEx failed: {hr:?}"));
        } else {
            trace!("WU COM apartment already initialized with a different mode");
            ComApartmentGuard {
                should_uninitialize: false,
            }
        };

        let step_start = Instant::now();
        trace!("WU COM CLSIDFromProgID Microsoft.Update.Session starting");
        let clsid = CLSIDFromProgID(&HSTRING::from("Microsoft.Update.Session"))
            .context("resolve Microsoft.Update.Session CLSID")?;
        trace!(
            elapsed_ms = step_start.elapsed().as_millis(),
            "WU COM CLSIDFromProgID Microsoft.Update.Session completed"
        );

        let step_start = Instant::now();
        trace!("WU COM CoCreateInstance Microsoft.Update.Session starting");
        let session: IUpdateSession = CoCreateInstance(&clsid, None, CLSCTX_INPROC_SERVER)
            .context("create Microsoft.Update.Session COM instance")?;
        trace!(
            elapsed_ms = step_start.elapsed().as_millis(),
            "WU COM CoCreateInstance Microsoft.Update.Session completed"
        );

        let step_start = Instant::now();
        trace!("WU COM CreateUpdateSearcher starting");
        let searcher = session
            .CreateUpdateSearcher()
            .context("create Windows Update searcher")?;
        trace!(
            elapsed_ms = step_start.elapsed().as_millis(),
            "WU COM CreateUpdateSearcher completed"
        );

        let step_start = Instant::now();
        trace!("WU COM SetServerSelection starting");
        match searcher.SetServerSelection(ServerSelection(0)) {
            Ok(_) => trace!(
                elapsed_ms = step_start.elapsed().as_millis(),
                "WU COM SetServerSelection completed"
            ),
            Err(e) => warn!(
                elapsed_ms = step_start.elapsed().as_millis(),
                error = %e,
                "WU COM SetServerSelection failed; continuing"
            ),
        }

        let step_start = Instant::now();
        trace!("WU COM SetOnline starting");
        match searcher.SetOnline(VARIANT_BOOL(-1)) {
            Ok(_) => trace!(
                elapsed_ms = step_start.elapsed().as_millis(),
                online = true,
                "WU COM SetOnline completed"
            ),
            Err(e) => warn!(
                elapsed_ms = step_start.elapsed().as_millis(),
                error = %e,
                "WU COM SetOnline failed; continuing"
            ),
        }

        let software_pending = query_pending_updates(
            &searcher,
            "software_pending",
            "IsInstalled=0 and IsHidden=0 and Type='Software'",
        )
        .unwrap_or_else(|e| {
            warn!(error = %e, "WU software pending query failed");
            Vec::new()
        });
        let driver_pending = query_pending_updates(
            &searcher,
            "driver_pending",
            "IsInstalled=0 and IsHidden=0 and Type='Driver'",
        )
        .unwrap_or_else(|e| {
            warn!(error = %e, "WU driver pending query failed");
            Vec::new()
        });
        let upgrade_pending = query_pending_updates(
            &searcher,
            "upgrade_pending",
            &format!(
                "IsInstalled=0 and IsHidden=0 and CategoryIDs contains '{}'",
                WU_UPGRADES_CATEGORY_ID
            ),
        )
        .unwrap_or_else(|e| {
            warn!(error = %e, "WU upgrade pending query failed");
            Vec::new()
        });
        let mut all_pending = Vec::new();
        extend_pending_unique(&mut all_pending, software_pending);
        extend_pending_unique(&mut all_pending, upgrade_pending);
        extend_pending_unique(&mut all_pending, driver_pending);

        let optional_updates = query_update_titles(
            &searcher,
            "optional_updates",
            "IsInstalled=0 and IsHidden=0 and BrowseOnly=1",
        )
        .unwrap_or_else(|e| {
            warn!(error = %e, "WU optional updates query failed");
            Vec::new()
        });

        let driver_updates = query_update_titles(
            &searcher,
            "driver_updates",
            "IsInstalled=0 and IsHidden=0 and Type='Driver'",
        )
        .unwrap_or_else(|e| {
            warn!(error = %e, "WU driver updates query failed");
            Vec::new()
        });

        debug!(
            elapsed_ms = started.elapsed().as_millis(),
            pending_updates = all_pending.len(),
            optional_updates = optional_updates.len(),
            driver_updates = driver_updates.len(),
            "WU COM update searches completed"
        );

        let mut status = NativeWindowsUpdateStatus {
            pending_update_count: all_pending.len() as u32,
            pending_updates: all_pending,
            optional_updates,
            driver_updates,
            history: Vec::new(),
            last_search_time: Some(Utc::now()),
        };

        let step_start = Instant::now();
        trace!("WU COM GetTotalHistoryCount starting");
        let total_history = match searcher.GetTotalHistoryCount() {
            Ok(v) => {
                trace!(
                    elapsed_ms = step_start.elapsed().as_millis(),
                    total_history = v,
                    "WU COM GetTotalHistoryCount completed"
                );
                v
            }
            Err(e) => {
                warn!(
                    elapsed_ms = step_start.elapsed().as_millis(),
                    error = %e,
                    "WU history count query failed"
                );
                0
            }
        };
        let history_count = total_history.clamp(0, 50);
        if history_count > 0 {
            let step_start = Instant::now();
            trace!(history_count, "WU COM QueryHistory starting");
            let history = match searcher.QueryHistory(0, history_count) {
                Ok(h) => {
                    trace!(
                        elapsed_ms = step_start.elapsed().as_millis(),
                        history_count,
                        "WU COM QueryHistory completed"
                    );
                    h
                }
                Err(e) => {
                    warn!(
                        elapsed_ms = step_start.elapsed().as_millis(),
                        history_count,
                        error = %e,
                        "WU history query failed"
                    );
                    let _ = guard;
                    return Ok(status);
                }
            };
            let count = history.Count().unwrap_or(0);
            trace!(
                history_items = count,
                "WU COM history collection count read"
            );
            for idx in 0..count {
                let entry = match history.get_Item(idx) {
                    Ok(v) => v,
                    Err(e) => {
                        warn!(idx, error = %e, "WU history item read failed");
                        continue;
                    }
                };
                if let Some(converted) = convert_history_entry(&entry) {
                    trace!(
                        idx,
                        title = %converted.title,
                        operation = %converted.operation,
                        result = %converted.result,
                        date = %converted.date,
                        "WU COM history item converted"
                    );
                    status.history.push(converted);
                }
            }
            status.history.sort_by(|a, b| b.date.cmp(&a.date));
        }

        debug!(
            elapsed_ms = started.elapsed().as_millis(),
            pending_update_count = status.pending_update_count,
            pending_updates = status.pending_updates.len(),
            optional_updates = status.optional_updates.len(),
            driver_updates = status.driver_updates.len(),
            history = status.history.len(),
            "Windows Update COM collection completed"
        );

        let _ = guard;
        Ok(status)
    }
}

fn get_cached_status() -> Option<NativeWindowsUpdateStatus> {
    let cache = WU_STATUS_CACHE.get_or_init(|| Mutex::new(None));
    let guard = cache.lock().ok()?;
    let (ts, status) = guard.as_ref()?;
    if ts.elapsed() <= WU_STATUS_CACHE_TTL {
        return Some(status.clone());
    }
    None
}

fn set_cached_status(status: NativeWindowsUpdateStatus) {
    let cache = WU_STATUS_CACHE.get_or_init(|| Mutex::new(None));
    if let Ok(mut guard) = cache.lock() {
        *guard = Some((Instant::now(), status));
    }
}

#[cfg(windows)]
fn pending_update_dedupe_key(update: &NativePendingUpdate) -> String {
    format!(
        "{}|{}",
        update
            .title
            .trim()
            .to_lowercase()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" "),
        update.kb.as_deref().unwrap_or("").trim().to_lowercase()
    )
}

#[cfg(windows)]
fn extend_pending_unique(target: &mut Vec<NativePendingUpdate>, updates: Vec<NativePendingUpdate>) {
    let mut seen = target
        .iter()
        .map(pending_update_dedupe_key)
        .collect::<HashSet<_>>();

    for update in updates {
        let key = pending_update_dedupe_key(&update);
        if seen.insert(key) {
            target.push(update);
        }
    }
}

#[cfg(windows)]
fn convert_pending_update(
    update: &windows::Win32::System::UpdateAgent::IUpdate,
) -> Result<NativePendingUpdate> {
    unsafe {
        let title = update.Title()?.to_string();
        let kb_collection = update.KBArticleIDs().ok();
        let kb = kb_collection.and_then(|c| c.get_Item(0).ok()).map(|s| {
            let value = s.to_string();
            if value.starts_with("KB") {
                value
            } else {
                format!("KB{}", value)
            }
        });

        let is_mandatory = variant_true(update.IsMandatory().unwrap_or(VARIANT_BOOL(0)));
        let size = None;
        let reboot_required = update
            .InstallationBehavior()
            .ok()
            .and_then(|b| b.RebootBehavior().ok())
            .map(|v| v.0 != 0)
            .unwrap_or(false);

        Ok(NativePendingUpdate {
            title,
            kb,
            is_mandatory,
            size,
            reboot_required,
        })
    }
}

#[cfg(windows)]
fn convert_history_entry(
    entry: &windows::Win32::System::UpdateAgent::IUpdateHistoryEntry,
) -> Option<NativeUpdateHistory> {
    unsafe {
        let title = entry.Title().ok()?.to_string();
        let date = entry
            .Date()
            .ok()
            .and_then(ole_automation_date_to_utc)
            .unwrap_or_else(Utc::now);
        let operation = entry
            .Operation()
            .ok()
            .map(map_history_operation)
            .unwrap_or_else(|| "Unknown".to_string());
        let result = entry
            .ResultCode()
            .ok()
            .map(map_history_result)
            .unwrap_or_else(|| "Unknown".to_string());
        Some(NativeUpdateHistory {
            date,
            title,
            operation,
            result,
        })
    }
}

#[cfg(windows)]
fn ole_automation_date_to_utc(value: f64) -> Option<DateTime<Utc>> {
    // OLE Automation DATE epoch is 1899-12-30.
    let base = NaiveDate::from_ymd_opt(1899, 12, 30)?.and_hms_opt(0, 0, 0)?;
    let secs = (value * 86_400.0).round() as i64;
    let dt = base.checked_add_signed(chrono::TimeDelta::seconds(secs))?;
    Some(DateTime::from_naive_utc_and_offset(dt, Utc))
}

#[cfg(windows)]
fn map_history_operation(op: windows::Win32::System::UpdateAgent::UpdateOperation) -> String {
    match op.0 {
        1 => "Installation".to_string(),
        2 => "Uninstallation".to_string(),
        _ => "Other".to_string(),
    }
}

#[cfg(windows)]
fn map_history_result(code: windows::Win32::System::UpdateAgent::OperationResultCode) -> String {
    match code.0 {
        2 => "Succeeded".to_string(),
        3 => "SucceededWithErrors".to_string(),
        4 => "Failed".to_string(),
        5 => "Aborted".to_string(),
        _ => "InProgress".to_string(),
    }
}

#[cfg(windows)]
fn variant_true(value: VARIANT_BOOL) -> bool {
    value.0 != 0
}

#[cfg(windows)]
fn query_pending_updates(
    searcher: &IUpdateSearcher,
    label: &str,
    criteria: &str,
) -> Result<Vec<NativePendingUpdate>> {
    unsafe {
        let started = Instant::now();
        trace!(label, criteria, "WU COM pending update search starting");
        let result = searcher
            .Search(&BSTR::from(criteria))
            .with_context(|| format!("WUA search failed for {label}: {criteria}"))?;
        trace!(
            label,
            elapsed_ms = started.elapsed().as_millis(),
            "WU COM pending update search completed"
        );

        let updates = result
            .Updates()
            .with_context(|| format!("WUA updates collection read failed for {label}"))?;
        let count = updates
            .Count()
            .with_context(|| format!("WUA updates count read failed for {label}"))?
            .max(0);
        trace!(label, count, "WU COM pending update count read");

        let mut pending = Vec::with_capacity(count as usize);
        for idx in 0..count {
            let update = updates.get_Item(idx).with_context(|| {
                format!("WUA pending update item read failed for {label} at {idx}")
            })?;
            let converted = convert_pending_update(&update).with_context(|| {
                format!("WUA pending update conversion failed for {label} at {idx}")
            })?;
            trace!(
                label,
                idx,
                title = %converted.title,
                kb = ?converted.kb,
                is_mandatory = converted.is_mandatory,
                reboot_required = converted.reboot_required,
                "WU COM pending update converted"
            );
            pending.push(converted);
        }
        debug!(
            label,
            elapsed_ms = started.elapsed().as_millis(),
            count = pending.len(),
            "WU COM pending update query completed"
        );
        Ok(pending)
    }
}

#[cfg(windows)]
fn query_update_titles(
    searcher: &IUpdateSearcher,
    label: &str,
    criteria: &str,
) -> Result<Vec<String>> {
    unsafe {
        let started = Instant::now();
        trace!(label, criteria, "WU COM title search starting");
        let result = searcher
            .Search(&BSTR::from(criteria))
            .with_context(|| format!("WUA title search failed for {label}: {criteria}"))?;
        trace!(
            label,
            elapsed_ms = started.elapsed().as_millis(),
            "WU COM title search completed"
        );

        let updates = result
            .Updates()
            .with_context(|| format!("WUA title updates collection read failed for {label}"))?;
        let count = updates
            .Count()
            .with_context(|| format!("WUA title updates count read failed for {label}"))?
            .max(0);
        trace!(label, count, "WU COM title update count read");

        let mut titles = Vec::with_capacity(count as usize);
        for idx in 0..count {
            let update = updates.get_Item(idx).with_context(|| {
                format!("WUA title update item read failed for {label} at {idx}")
            })?;
            let title = update
                .Title()
                .with_context(|| format!("WUA title read failed for {label} at {idx}"))?
                .to_string();
            trace!(label, idx, title = %title, "WU COM title update read");
            titles.push(title);
        }
        debug!(
            label,
            elapsed_ms = started.elapsed().as_millis(),
            count = titles.len(),
            "WU COM title query completed"
        );
        Ok(titles)
    }
}
