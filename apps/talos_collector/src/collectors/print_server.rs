use crate::collectors::Collector;
use crate::models::{PrintServerDetails, PrinterInfo, PrintersInfo};
use crate::windows_utils::registry::RegistryHelper;
use crate::windows_utils::wmi::WmiHelper;
use anyhow::Result;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use tracing::debug;

pub struct PrintServerCollector;

#[async_trait]
impl Collector for PrintServerCollector {
    fn name(&self) -> &'static str {
        "Printers"
    }

    fn data_type(&self) -> &'static str {
        "printers"
    }

    fn estimated_duration_ms(&self) -> u64 {
        3000
    }

    async fn collect(&self) -> Result<Value> {
        debug!("Starting Printers collection");

        let mut out = PrintersInfo {
            printers: Vec::new(),
            print_server: PrintServerDetails::default(),
        };
        let printers = WmiHelper::query_values("SELECT Name, ShareName, DriverName, PortName, PrinterStatus, Shared, Jobs FROM Win32_Printer")
            .await
            .unwrap_or_default();
        let jobs =
            WmiHelper::query_values("SELECT Name, DriverName, PortName FROM Win32_PrinterDriver")
                .await
                .unwrap_or_default();
        let ports = WmiHelper::query_values("SELECT Name FROM Win32_TCPIPPrinterPort")
            .await
            .unwrap_or_default();
        let print_jobs = WmiHelper::query_values("SELECT Name FROM Win32_PrintJob")
            .await
            .unwrap_or_default();

        out.print_server.pending_jobs = print_jobs.len() as u32;

        let mut driver_names = HashSet::new();
        for d in jobs {
            if let Some(name) = d.get("Name").and_then(|v| v.as_str()) {
                driver_names.insert(name.to_string());
            }
        }
        out.print_server.drivers = driver_names.into_iter().collect();

        out.print_server.ports = ports
            .into_iter()
            .filter_map(|p| {
                p.get("Name")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
            })
            .collect();

        for p in printers {
            let status = match p.get("PrinterStatus").and_then(|v| v.as_u64()).unwrap_or(0) {
                3 => "Idle",
                4 => "Printing",
                5 => "Warmup",
                7 => "Offline",
                _ => "Unknown",
            }
            .to_string();

            out.printers.push(PrinterInfo {
                name: p
                    .get("Name")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string(),
                share_name: p
                    .get("ShareName")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string()),
                driver_name: p
                    .get("DriverName")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string(),
                port_name: p
                    .get("PortName")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string(),
                status,
                is_shared: p.get("Shared").and_then(|v| v.as_bool()).unwrap_or(false),
                job_count: p.get("Jobs").and_then(|v| v.as_u64()).unwrap_or(0) as u32,
            });
        }

        // Fallback/augmentation via registry to capture local virtual printers
        // even when WMI returns empty or partially populated data.
        self.merge_registry_printers(&mut out.printers);

        debug!("Printers collection completed");
        Ok(json!(out))
    }
}

impl PrintServerCollector {
    fn merge_registry_printers(&self, printers: &mut Vec<PrinterInfo>) {
        let base = r"SYSTEM\CurrentControlSet\Control\Print\Printers";
        let subkeys = RegistryHelper::enum_subkeys("HKLM", base).unwrap_or_default();

        let mut by_name: HashMap<String, usize> = HashMap::new();
        for (idx, p) in printers.iter().enumerate() {
            by_name.insert(p.name.to_ascii_lowercase(), idx);
        }

        for printer_name in subkeys {
            let key = format!(r"{}\{}", base, printer_name);
            let driver = RegistryHelper::read_string("HKLM", &key, "Printer Driver")
                .ok()
                .flatten()
                .unwrap_or_default();
            let port = RegistryHelper::read_string("HKLM", &key, "Port")
                .ok()
                .flatten()
                .unwrap_or_default();
            let share_name = RegistryHelper::read_string("HKLM", &key, "Share Name")
                .ok()
                .flatten()
                .filter(|s| !s.is_empty());
            let shared = RegistryHelper::read_dword("HKLM", &key, "Attributes")
                .ok()
                .flatten()
                .map(|v| (v & 0x8) != 0)
                .unwrap_or(false);

            if let Some(existing_idx) = by_name.get(&printer_name.to_ascii_lowercase()).copied() {
                let existing = &mut printers[existing_idx];
                if existing.driver_name.is_empty() {
                    existing.driver_name = driver;
                }
                if existing.port_name.is_empty() {
                    existing.port_name = port;
                }
                if existing.share_name.is_none() {
                    existing.share_name = share_name;
                }
            } else {
                let idx = printers.len();
                printers.push(PrinterInfo {
                    name: printer_name.clone(),
                    share_name,
                    driver_name: driver,
                    port_name: port,
                    status: "Unknown".to_string(),
                    is_shared: shared,
                    job_count: 0,
                });
                by_name.insert(printer_name.to_ascii_lowercase(), idx);
            }
        }
    }
}
