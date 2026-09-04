use crate::collectors::Collector;
use crate::models::{
    BatteryInfo, CpuInfo, DiskInfo, GpuInfo, HardwareInfo, MemoryInfo, MemoryModule,
    MotherboardInfo, NetworkAdapterHardware, SmartInfo, TpmInfo, VolumeInfo,
};
use crate::windows_utils::{registry::RegistryHelper, wmi::WmiHelper};
use anyhow::Result;
use async_trait::async_trait;
use serde_json::{json, Value};
use sysinfo::System;
use tracing::debug;

pub struct HardwareCollector;

#[async_trait]
impl Collector for HardwareCollector {
    fn name(&self) -> &'static str {
        "Hardware"
    }

    fn data_type(&self) -> &'static str {
        "hardware"
    }

    fn estimated_duration_ms(&self) -> u64 {
        3000
    }

    fn requires_admin(&self) -> bool {
        false // Most info available without admin, some SMART data might need it
    }

    async fn collect(&self) -> Result<Value> {
        debug!("Starting Hardware collection");

        let mut hw = HardwareInfo::default();

        // Get CPU info
        hw.cpu = self.collect_cpu_info().await?;

        // Get memory info
        hw.memory = self.collect_memory_info().await?;

        // Get disk info
        hw.disks = self.collect_disk_info().await?;

        // Get GPU info
        hw.gpus = self.collect_gpu_info().await?;

        // Get network adapter hardware info
        hw.network_adapters = self.collect_network_adapters().await?;

        // Get TPM info (optional - don't fail if this errors)
        hw.tpm = self.collect_tpm_info().await.ok();

        // Get Secure Boot status (optional)
        hw.secure_boot = self.get_secure_boot_status().await.ok();

        // Get battery info (optional)
        hw.battery = self.collect_battery_info().await.ok();

        // Get motherboard info (optional)
        hw.motherboard = self.collect_motherboard_info().await.ok();
        hw.todo_data_collection.push(
            "TODO: volume.is_bitlocker_encrypted is not implemented yet in Hardware collector."
                .to_string(),
        );

        debug!("Hardware collection completed");

        Ok(json!(hw))
    }
}

impl HardwareCollector {
    async fn collect_cpu_info(&self) -> Result<CpuInfo> {
        let sys = System::new_all();
        let cpus = sys.cpus();

        let mut cpu = CpuInfo::default();

        if let Some(first_cpu) = cpus.first() {
            cpu.brand = first_cpu.brand().to_string();
            cpu.frequency_mhz = first_cpu.frequency();
        }

        cpu.cores = sys.physical_core_count().unwrap_or(cpus.len()) as u32;
        cpu.threads = cpus.len() as u32;

        // Get additional info from WMI
        let wmi_cpus = WmiHelper::get_processor_info()
            .await
            .map_err(|e| anyhow::anyhow!("WMI get_processor_info failed: {}", e))?;
        if let Some(first) = wmi_cpus.first() {
            if let Some(manuf) = first.get("Manufacturer").and_then(|v| v.as_str()) {
                cpu.manufacturer = manuf.to_string();
            }
            if let Some(socket) = first.get("SocketDesignation").and_then(|v| v.as_str()) {
                cpu.socket = socket.to_string();
            }
            if let Some(processor_id) = first.get("ProcessorId").and_then(|v| v.as_str()) {
                cpu.processor_id = processor_id.to_string();
            }
        }

        Ok(cpu)
    }

    async fn collect_memory_info(&self) -> Result<MemoryInfo> {
        let sys = System::new_all();

        let mut mem = MemoryInfo {
            total_bytes: sys.total_memory(),
            available_bytes: sys.available_memory(),
            ..Default::default()
        };

        // Get detailed memory info from WMI
        let wmi_mem = WmiHelper::get_memory_info().await?;
        mem.modules = wmi_mem
            .iter()
            .map(|m| MemoryModule {
                slot: m
                    .get("DeviceLocator")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
                    .unwrap_or_default(),
                capacity_bytes: WmiHelper::parse_wmi_size(m.get("Capacity").unwrap_or(&json!(0)))
                    .unwrap_or(0),
                speed_mhz: m.get("Speed").and_then(|v| v.as_u64()).unwrap_or(0) as u32,
                manufacturer: m
                    .get("Manufacturer")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
                    .unwrap_or_default(),
                part_number: m
                    .get("PartNumber")
                    .and_then(|v| v.as_str())
                    .map(|s| s.trim().to_string())
                    .unwrap_or_default(),
            })
            .collect();

        mem.slots_used = mem.modules.len() as u32;

        if let Some(first) = wmi_mem.first() {
            mem.speed_mhz = first.get("Speed").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
        }

        Ok(mem)
    }

    async fn collect_disk_info(&self) -> Result<Vec<DiskInfo>> {
        let mut disks = Vec::new();

        let wmi_disks = WmiHelper::get_disk_drives().await?;
        let wmi_volumes = WmiHelper::get_logical_disks().await?;

        for disk_drive in wmi_disks.iter() {
            let mut disk = DiskInfo {
                device_id: disk_drive
                    .get("DeviceID")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
                    .unwrap_or_default(),
                model: disk_drive
                    .get("Model")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
                    .unwrap_or_default(),
                serial_number: disk_drive
                    .get("SerialNumber")
                    .and_then(|v| v.as_str())
                    .map(|s| s.trim().to_string())
                    .unwrap_or_default(),
                interface: disk_drive
                    .get("InterfaceType")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| "Unknown".to_string()),
                media_type: disk_drive
                    .get("MediaType")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| "Unknown".to_string()),
                size_bytes: WmiHelper::parse_wmi_size(disk_drive.get("Size").unwrap_or(&json!(0)))
                    .unwrap_or(0),
                smart: None,
                volumes: Vec::new(),
            };

            // Try to get SMART data (optional)
            disk.smart = self.get_smart_data(&disk.device_id).await.ok();

            // Get associated volumes
            let disk_index = disk_drive
                .get("Index")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            disk.volumes = self.get_volumes_for_disk(disk_index, &wmi_volumes).await;

            disks.push(disk);
        }

        Ok(disks)
    }

    async fn get_smart_data(&self, device_id: &str) -> Result<SmartInfo> {
        let mut smart = SmartInfo::default();
        smart.health_status = "Unknown".to_string();

        // Try WMI SMART data
        if let Ok(status) = WmiHelper::get_smart_failure_status(device_id).await {
            if let Some(s) = status
                .iter()
                .find(|row| self.instance_matches_device(row, device_id))
                .or_else(|| status.first())
            {
                smart.health_status = if s
                    .get("PredictFailure")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false)
                {
                    "Predicted Failure".to_string()
                } else {
                    "OK".to_string()
                };
            }
        }

        // Parse SMART attributes (ATA vendor-specific payload)
        if let Ok(attrs) = WmiHelper::get_smart_failure_data().await {
            if let Some(attr_row) = attrs
                .iter()
                .find(|row| self.instance_matches_device(row, device_id))
                .or_else(|| attrs.first())
            {
                if let Some(vendor) = self.extract_vendor_specific_bytes(attr_row) {
                    self.apply_smart_attributes(&vendor, &mut smart);
                }
            }
        }

        // Temperature can also be exposed through the ATAPI smart payload.
        if let Ok(temps) = WmiHelper::get_smart_temperature_data().await {
            if let Some(temp_row) = temps
                .iter()
                .find(|row| self.instance_matches_device(row, device_id))
                .or_else(|| temps.first())
            {
                if let Some(vendor) = self.extract_vendor_specific_bytes(temp_row) {
                    let parsed_temp = self.parse_temperature_from_vendor_bytes(&vendor);
                    if smart.temperature_c.is_none() {
                        smart.temperature_c = parsed_temp;
                    }
                }
            }
        }

        // If health not explicitly provided by PredictFailure, infer a sane value.
        if smart.health_status == "Unknown"
            && (smart.reallocated_sectors.unwrap_or(0) > 0
                || smart.pending_sectors.unwrap_or(0) > 0)
        {
            smart.health_status = "Warning".to_string();
        }

        Ok(smart)
    }

    fn instance_matches_device(&self, row: &Value, device_id: &str) -> bool {
        let instance = row
            .get("InstanceName")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        if instance.is_empty() {
            return false;
        }

        // Win32_DiskDrive DeviceID usually looks like "\\.\PHYSICALDRIVE0";
        // smart instance names typically include "physicaldrive0".
        let mut normalized = device_id.to_ascii_lowercase();
        normalized = normalized.replace(r"\\.\", "");
        instance.contains(&normalized)
    }

    fn extract_vendor_specific_bytes(&self, row: &Value) -> Option<Vec<u8>> {
        let field = row.get("VendorSpecific")?;
        if let Some(bytes) = field.as_array() {
            let mut out = Vec::with_capacity(bytes.len());
            for v in bytes {
                if let Some(n) = v.as_u64() {
                    out.push((n & 0xFF) as u8);
                } else if let Some(n) = v.as_i64() {
                    out.push((n & 0xFF) as u8);
                }
            }
            if !out.is_empty() {
                return Some(out);
            }
        }
        None
    }

    fn apply_smart_attributes(&self, vendor: &[u8], smart: &mut SmartInfo) {
        // ATA SMART entries are 12-byte slots starting at byte 2.
        // We parse common IDs when present.
        let mut offset = 2usize;
        while offset + 11 < vendor.len() {
            let attr_id = vendor[offset];
            if attr_id == 0 {
                offset += 12;
                continue;
            }

            let raw = &vendor[offset + 5..offset + 11];
            match attr_id {
                5 => smart.reallocated_sectors = Some(self.raw_le_u48(raw) as u32),
                9 => smart.power_on_hours = Some(self.raw_le_u48(raw)),
                190 | 194 => {
                    if smart.temperature_c.is_none() {
                        let temp = raw[0] as i32;
                        if (0..=120).contains(&temp) {
                            smart.temperature_c = Some(temp);
                        }
                    }
                }
                197 => smart.pending_sectors = Some(self.raw_le_u48(raw) as u32),
                // SSD wear/health style attributes vary by vendor.
                // 202: percentage used (some drives), 231/233: life left/used proxies.
                202 | 231 | 233 => {
                    let value = raw[0] as u32;
                    if value <= 100 {
                        smart.wear_level = Some(value);
                    }
                }
                _ => {}
            }

            offset += 12;
        }
    }

    fn parse_temperature_from_vendor_bytes(&self, vendor: &[u8]) -> Option<i32> {
        let mut offset = 2usize;
        while offset + 11 < vendor.len() {
            let attr_id = vendor[offset];
            if attr_id == 190 || attr_id == 194 {
                let raw = &vendor[offset + 5..offset + 11];
                let temp = raw[0] as i32;
                if (0..=120).contains(&temp) {
                    return Some(temp);
                }
            }
            offset += 12;
        }
        None
    }

    fn raw_le_u48(&self, raw: &[u8]) -> u64 {
        raw.iter()
            .enumerate()
            .fold(0u64, |acc, (i, b)| acc | ((*b as u64) << (8 * i)))
    }

    async fn get_volumes_for_disk(
        &self,
        _disk_index: u64,
        wmi_volumes: &[Value],
    ) -> Vec<VolumeInfo> {
        let mut volumes = Vec::new();

        for vol in wmi_volumes {
            // Check if this volume belongs to the disk via disk partition association
            // This is a simplified check - WMI association queries would be more accurate
            let drive_letter = vol
                .get("DeviceID")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .unwrap_or_default();

            if !drive_letter.is_empty() {
                let total =
                    WmiHelper::parse_wmi_size(vol.get("Size").unwrap_or(&json!(0))).unwrap_or(0);
                let free = WmiHelper::parse_wmi_size(vol.get("FreeSpace").unwrap_or(&json!(0)))
                    .unwrap_or(0);

                volumes.push(VolumeInfo {
                    drive_letter: drive_letter.clone(),
                    label: vol
                        .get("VolumeName")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string())
                        .unwrap_or_default(),
                    filesystem: vol
                        .get("FileSystem")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| "Unknown".to_string()),
                    total_bytes: total,
                    free_bytes: free,
                    percent_used: if total > 0 {
                        ((total - free) as f64 / total as f64 * 100.0) as f32
                    } else {
                        0.0
                    },
                    is_bitlocker_encrypted: None, // Would need BitLocker WMI
                });
            }
        }

        volumes
    }

    async fn collect_gpu_info(&self) -> Result<Vec<GpuInfo>> {
        let mut gpus = Vec::new();

        if let Ok(wmi_gpus) = WmiHelper::get_gpu_info().await {
            for gpu in wmi_gpus {
                gpus.push(GpuInfo {
                    name: gpu
                        .get("Name")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| "Unknown".to_string()),
                    manufacturer: gpu
                        .get("AdapterCompatibility")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string())
                        .unwrap_or_default(),
                    adapter_ram_bytes: WmiHelper::parse_wmi_size(
                        gpu.get("AdapterRAM").unwrap_or(&json!(0)),
                    )
                    .unwrap_or(0),
                    driver_version: gpu
                        .get("DriverVersion")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string())
                        .unwrap_or_default(),
                    driver_date: gpu
                        .get("DriverDate")
                        .and_then(|v| v.as_str())
                        .and_then(WmiHelper::parse_wmi_datetime_str)
                        .map(|dt| dt.format("%Y-%m-%d").to_string()),
                    video_mode: gpu
                        .get("VideoModeDescription")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string())
                        .unwrap_or_default(),
                });
            }
        }

        Ok(gpus)
    }

    async fn collect_network_adapters(&self) -> Result<Vec<NetworkAdapterHardware>> {
        let mut adapters = Vec::new();

        if let Ok(wmi_adapters) = WmiHelper::get_network_adapters().await {
            for adapter in wmi_adapters {
                let adapter_type = adapter
                    .get("AdapterType")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| "Unknown".to_string());

                let is_virtual = adapter
                    .get("PNPDeviceID")
                    .and_then(|v| v.as_str())
                    .map(|s| s.contains("ROOT\\") || s.contains("VIRTUAL") || s.contains("HYPER-V"))
                    .unwrap_or(false);

                let is_physical =
                    !is_virtual && adapter_type != "Tunnel" && adapter_type != "Software Loopback";

                adapters.push(NetworkAdapterHardware {
                    name: adapter
                        .get("Name")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| "Unknown".to_string()),
                    mac_address: adapter
                        .get("MACAddress")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string())
                        .unwrap_or_default(),
                    is_physical,
                    is_virtual,
                    adapter_type: adapter_type.clone(),
                    speed_mbps: adapter
                        .get("Speed")
                        .and_then(|v| v.as_u64())
                        .map(|s| s / 1_000_000), // Convert to Mbps
                });
            }
        }

        Ok(adapters)
    }

    async fn collect_tpm_info(&self) -> Result<TpmInfo> {
        let mut tpm = TpmInfo::default();

        if let Ok(Some(tpm_data)) = WmiHelper::get_tpm_info().await {
            tpm.present = true;

            if let Some(spec) = tpm_data.get("SpecVersion").and_then(|v| v.as_str()) {
                // Format: "2.0, 0, 0, 0" -> take first part
                tpm.version = spec.split(',').next().unwrap_or("2.0").trim().to_string();
            }

            tpm.ready = self
                .parse_tpm_bool(&tpm_data, &["IsReady_Information", "IsReady", "TpmReady"])
                .unwrap_or(true);
            tpm.enabled = self
                .parse_tpm_bool(
                    &tpm_data,
                    &[
                        "IsEnabled_Information",
                        "IsEnabled_InitialValue",
                        "IsEnabled",
                        "TpmEnabled",
                    ],
                )
                .unwrap_or(false);
            tpm.activated = self
                .parse_tpm_bool(
                    &tpm_data,
                    &[
                        "IsActivated_Information",
                        "IsActivated_InitialValue",
                        "IsActivated",
                        "TpmActivated",
                    ],
                )
                .unwrap_or(false);
            tpm.owned = self
                .parse_tpm_bool(
                    &tpm_data,
                    &[
                        "IsOwned_Information",
                        "IsOwned_InitialValue",
                        "IsOwned",
                        "TpmOwned",
                    ],
                )
                .unwrap_or(false);
        }

        Ok(tpm)
    }

    fn parse_tpm_bool(&self, data: &Value, keys: &[&str]) -> Option<bool> {
        for key in keys {
            if let Some(v) = data.get(*key) {
                if let Some(b) = v.as_bool() {
                    return Some(b);
                }
                if let Some(n) = v.as_u64() {
                    // TPM _Information fields often encode 0 as success/true.
                    return Some(if key.ends_with("_Information") {
                        n == 0
                    } else {
                        n != 0
                    });
                }
                if let Some(s) = v.as_str() {
                    match s.to_ascii_lowercase().as_str() {
                        "true" | "1" | "yes" => return Some(true),
                        "false" | "0" | "no" => return Some(false),
                        _ => {}
                    }
                }
            }
        }
        None
    }

    async fn get_secure_boot_status(&self) -> Result<bool> {
        // Check registry for Secure Boot status
        if let Ok(Some(val)) = RegistryHelper::read_dword(
            "HKLM",
            r"SYSTEM\CurrentControlSet\Control\SecureBoot\State",
            "UEFISecureBootEnabled",
        ) {
            return Ok(val != 0);
        }

        // Fallback to Windows API
        if let Some(status) = crate::windows_utils::winapi_helpers::get_secure_boot_status() {
            return Ok(status);
        }

        Err(anyhow::anyhow!("Could not determine Secure Boot status"))
    }

    async fn collect_battery_info(&self) -> Result<BatteryInfo> {
        let mut battery = BatteryInfo::default();
        battery.present = false;

        // Query WMI for battery
        if let Ok(battery_info) = WmiHelper::get_battery_info().await {
            if let Some(b) = &battery_info {
                battery.present = true;

                if let Some(status) = b.get("BatteryStatus").and_then(|v| v.as_u64()) {
                    battery.battery_status = match status {
                        1 => "Discharging".to_string(),
                        2 => "AC Power".to_string(),
                        3 => "Fully Charged".to_string(),
                        _ => format!("Status {}", status),
                    };
                }

                // Try to get more detailed battery info from Win32_PortableBattery
                if let Ok(portable) = WmiHelper::get_portable_battery().await {
                    if let Some(pb) = &portable {
                        if let Some(design_cap) = pb.get("DesignCapacity").and_then(|v| v.as_u64())
                        {
                            battery.design_capacity_mwh = Some(design_cap as u32);
                        }
                        if let Some(full_cap) =
                            pb.get("FullChargeCapacity").and_then(|v| v.as_u64())
                        {
                            battery.full_charge_capacity_mwh = Some(full_cap as u32);
                            if let Some(design) = battery.design_capacity_mwh {
                                battery.health_percent =
                                    Some(((full_cap as f64 / design as f64) * 100.0) as u32);
                            }
                        }
                    }
                }
            }
        }

        Ok(battery)
    }

    async fn collect_motherboard_info(&self) -> Result<MotherboardInfo> {
        let mut mb = MotherboardInfo::default();

        if let Ok(bios) = WmiHelper::get_bios_info().await {
            if let Some(manuf) = bios.get("Manufacturer").and_then(|v| v.as_str()) {
                mb.manufacturer = manuf.to_string();
            }
            if let Some(product) = bios.get("Name").and_then(|v| v.as_str()) {
                mb.product = product.to_string();
            }
            if let Some(serial) = bios.get("SerialNumber").and_then(|v| v.as_str()) {
                mb.serial_number = serial.to_string();
            }
            if let Some(version) = bios.get("Version").and_then(|v| v.as_str()) {
                mb.bios_version = version.to_string();
            }
            if let Some(date) = bios.get("ReleaseDate").and_then(|v| v.as_str()) {
                mb.bios_date = WmiHelper::parse_wmi_datetime_str(date)
                    .map(|dt| dt.format("%Y-%m-%d").to_string());
            }
        }

        // Get system info for more motherboard details
        if let Ok(cs) = WmiHelper::get_computer_info().await {
            if mb.manufacturer.is_empty() {
                mb.manufacturer = cs
                    .get("Manufacturer")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
                    .unwrap_or_default();
            }
            if mb.product.is_empty() {
                mb.product = cs
                    .get("Model")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
                    .unwrap_or_default();
            }
        }

        Ok(mb)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hardware_collector_name() {
        let collector = HardwareCollector;
        assert_eq!(collector.name(), "Hardware");
    }
}
