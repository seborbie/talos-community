use crate::collectors::Collector;
use crate::models::{ScheduledTaskInfo, ScheduledTasksInfo};
use crate::windows_utils::registry::RegistryHelper;
use crate::windows_utils::wmi::WmiHelper;
#[cfg(windows)]
use anyhow::Context;
use anyhow::Result;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde_json::{json, Value};
use std::collections::HashSet;
use tracing::debug;

pub struct ScheduledTasksCollector;

#[async_trait]
impl Collector for ScheduledTasksCollector {
    fn name(&self) -> &'static str {
        "ScheduledTasks"
    }

    fn data_type(&self) -> &'static str {
        "scheduled_tasks"
    }

    fn estimated_duration_ms(&self) -> u64 {
        4000
    }

    async fn collect(&self) -> Result<Value> {
        debug!("Starting ScheduledTasks collection");

        let mut tasks_info = ScheduledTasksInfo::default();
        tasks_info.tasks = self.collect_tasks_native().await;

        debug!("ScheduledTasks collection completed");
        Ok(json!(tasks_info))
    }
}

impl ScheduledTasksCollector {
    async fn collect_tasks_native(&self) -> Vec<ScheduledTaskInfo> {
        let com_tasks = self.query_tasks_com().await;
        if !com_tasks.is_empty() {
            return com_tasks;
        }

        let wmi_tasks = self.query_tasks_wmi().await;
        let mapped: Vec<ScheduledTaskInfo> = wmi_tasks
            .into_iter()
            .filter_map(|task| self.map_wmi_task(task))
            .collect();
        if !mapped.is_empty() {
            return mapped;
        }

        self.collect_tasks_from_registry()
    }

    async fn query_tasks_wmi(&self) -> Vec<Value> {
        // Use SELECT * to avoid schema/property mismatch on some builds that can cause WBEM_E_INVALID_QUERY.
        if let Ok(rows) = WmiHelper::query_values_in_namespace(
            "ROOT\\Microsoft\\Windows\\TaskScheduler",
            "SELECT * FROM MSFT_ScheduledTask",
        )
        .await
        {
            if !rows.is_empty() {
                return rows;
            }
        }

        if let Ok(rows) = WmiHelper::query_values_in_namespace(
            "ROOT\\Microsoft\\Windows\\TaskScheduler",
            "SELECT * FROM PS_ScheduledTask",
        )
        .await
        {
            if !rows.is_empty() {
                return rows;
            }
        }

        // Legacy scheduler model fallback.
        WmiHelper::query_values("SELECT * FROM Win32_ScheduledJob")
            .await
            .unwrap_or_default()
    }

    fn map_wmi_task(&self, task: Value) -> Option<ScheduledTaskInfo> {
        let name = task
            .get("TaskName")
            .or_else(|| task.get("ElementName"))
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        if name.is_empty() {
            return None;
        }

        let path = task
            .get("TaskPath")
            .or_else(|| task.get("URI"))
            .and_then(|v| v.as_str())
            .unwrap_or("\\")
            .to_string();

        Some(ScheduledTaskInfo {
            name,
            path,
            state: task
                .get("State")
                .map(Self::map_task_state)
                .unwrap_or_else(|| "Unknown".to_string()),
            enabled: task
                .get("Enabled")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
            last_run_time: Self::parse_dt(task.get("LastRunTime")),
            next_run_time: Self::parse_dt(task.get("NextRunTime")),
            author: task
                .get("Author")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
        })
    }

    async fn query_tasks_com(&self) -> Vec<ScheduledTaskInfo> {
        #[cfg(windows)]
        {
            match tokio::task::spawn_blocking(Self::query_tasks_com_blocking)
                .await
                .context("Task Scheduler COM query task panicked")
            {
                Ok(Ok(tasks)) => tasks,
                _ => Vec::new(),
            }
        }
        #[cfg(not(windows))]
        {
            Vec::new()
        }
    }

    #[cfg(windows)]
    fn query_tasks_com_blocking() -> Result<Vec<ScheduledTaskInfo>> {
        use windows::core::{BSTR, VARIANT};
        use windows::Win32::Foundation::RPC_E_CHANGED_MODE;
        use windows::Win32::System::Com::{
            CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER,
            COINIT_MULTITHREADED,
        };
        use windows::Win32::System::TaskScheduler::{
            ITaskService, TaskScheduler, TASK_ENUM_HIDDEN,
        };

        unsafe {
            let init_result = CoInitializeEx(None, COINIT_MULTITHREADED);
            let mut should_uninitialize = false;
            if init_result.is_ok() {
                should_uninitialize = true;
            } else if init_result != RPC_E_CHANGED_MODE {
                return Err(anyhow::anyhow!(
                    "CoInitializeEx failed: 0x{:08X}",
                    init_result.0 as u32
                ));
            }

            let service: ITaskService =
                CoCreateInstance(&TaskScheduler, None, CLSCTX_INPROC_SERVER)?;
            let empty = VARIANT::default();
            service.Connect(&empty, &empty, &empty, &empty)?;

            let root = service.GetFolder(&BSTR::from("\\"))?;
            let mut tasks = Vec::new();
            Self::walk_com_folders(&root, &mut tasks, TASK_ENUM_HIDDEN)?;

            if should_uninitialize {
                CoUninitialize();
            }

            Ok(tasks)
        }
    }

    #[cfg(windows)]
    fn walk_com_folders(
        folder: &windows::Win32::System::TaskScheduler::ITaskFolder,
        out: &mut Vec<ScheduledTaskInfo>,
        task_flags: windows::Win32::System::TaskScheduler::TASK_ENUM_FLAGS,
    ) -> Result<()> {
        use windows::Win32::System::TaskScheduler::{TASK_STATE_READY, TASK_STATE_RUNNING};

        unsafe {
            let tasks = folder.GetTasks(task_flags.0)?;
            let task_count = tasks.Count()?;
            for i in 1..=task_count {
                let idx = windows::core::VARIANT::from(i);
                let task = tasks.get_Item(&idx)?;
                let full_path = task.Path()?.to_string();
                let (path, name) = split_task_path(&full_path);
                let state = match task.State()? {
                    TASK_STATE_RUNNING => "Running".to_string(),
                    TASK_STATE_READY => "Ready".to_string(),
                    s => format!("{:?}", s),
                };

                out.push(ScheduledTaskInfo {
                    name,
                    path,
                    state,
                    enabled: task.Enabled()?.as_bool(),
                    last_run_time: ole_date_to_utc(task.LastRunTime()?),
                    next_run_time: ole_date_to_utc(task.NextRunTime()?),
                    author: task.Definition().ok().and_then(|d| {
                        d.RegistrationInfo().ok().and_then(|r| {
                            let mut author = windows::core::BSTR::new();
                            if r.Author(&mut author).is_ok() {
                                let val = author.to_string();
                                if val.is_empty() {
                                    None
                                } else {
                                    Some(val)
                                }
                            } else {
                                None
                            }
                        })
                    }),
                });
            }

            let subfolders = folder.GetFolders(0)?;
            let folder_count = subfolders.Count()?;
            for i in 1..=folder_count {
                let idx = windows::core::VARIANT::from(i);
                let sub = subfolders.get_Item(&idx)?;
                Self::walk_com_folders(&sub, out, task_flags)?;
            }
        }

        Ok(())
    }

    fn map_task_state(value: &Value) -> String {
        if let Some(s) = value.as_str() {
            return s.to_string();
        }
        match value.as_u64().unwrap_or(0) {
            0 => "Unknown".to_string(),
            1 => "Disabled".to_string(),
            2 => "Queued".to_string(),
            3 => "Ready".to_string(),
            4 => "Running".to_string(),
            _ => "Unknown".to_string(),
        }
    }

    fn parse_dt(v: Option<&Value>) -> Option<DateTime<Utc>> {
        let s = v.and_then(|x| x.as_str())?;
        DateTime::parse_from_rfc3339(s)
            .ok()
            .map(|dt| dt.with_timezone(&Utc))
            .or_else(|| WmiHelper::parse_wmi_datetime_str(s))
    }

    fn collect_tasks_from_registry(&self) -> Vec<ScheduledTaskInfo> {
        let base = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion\Schedule\TaskCache\Tree";
        let mut tasks = Vec::new();
        let mut seen = HashSet::new();
        self.walk_registry_task_tree(base, "\\", &mut tasks, &mut seen);
        tasks
    }

    fn walk_registry_task_tree(
        &self,
        key: &str,
        logical_path: &str,
        out: &mut Vec<ScheduledTaskInfo>,
        seen: &mut HashSet<String>,
    ) {
        let subkeys = match RegistryHelper::enum_subkeys("HKLM", key) {
            Ok(keys) => keys,
            Err(_) => return,
        };

        for sub in subkeys {
            let child_key = format!(r"{}\{}", key, sub);
            let child_logical = if logical_path == "\\" {
                format!(r"\{}", sub)
            } else {
                format!(r"{}\{}", logical_path, sub)
            };

            if RegistryHelper::read_string("HKLM", &child_key, "Id")
                .ok()
                .flatten()
                .is_some()
                && seen.insert(child_logical.clone())
            {
                let (path, name) = split_task_path(&child_logical);
                let enabled = RegistryHelper::read_dword("HKLM", &child_key, "Enabled")
                    .ok()
                    .flatten()
                    .map(|v| v != 0)
                    .unwrap_or(true);
                out.push(ScheduledTaskInfo {
                    name,
                    path,
                    state: "Unknown".to_string(),
                    enabled,
                    last_run_time: None,
                    next_run_time: None,
                    author: None,
                });
            }

            self.walk_registry_task_tree(&child_key, &child_logical, out, seen);
        }
    }
}

fn split_task_path(full: &str) -> (String, String) {
    if let Some(pos) = full.rfind('\\') {
        let name = full[pos + 1..].to_string();
        let path = if pos == 0 {
            "\\".to_string()
        } else {
            full[..pos].to_string()
        };
        (path, name)
    } else {
        ("\\".to_string(), full.to_string())
    }
}

#[cfg(windows)]
fn ole_date_to_utc(ole_date: f64) -> Option<DateTime<Utc>> {
    // OLE Automation date origin: 1899-12-30
    if ole_date <= 0.0 {
        return None;
    }
    let base = chrono::NaiveDate::from_ymd_opt(1899, 12, 30)?.and_hms_opt(0, 0, 0)?;
    let seconds = (ole_date * 86_400.0) as i64;
    let dt = base + chrono::Duration::seconds(seconds);
    Some(DateTime::<Utc>::from_naive_utc_and_offset(dt, Utc))
}
