use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde_json::{json, Map, Value};
use sysinfo::{Disks, Networks, System};
use tokio::{process::Command, time::timeout};
use tracing::{debug, info};

const COMMAND_TIMEOUT: Duration = Duration::from_secs(15);
const PLUTIL_TIMEOUT: Duration = Duration::from_secs(3);
const SOFTWAREUPDATE_TIMEOUT: Duration = Duration::from_secs(45);
const SYSTEM_PROFILER_TIMEOUT: Duration = Duration::from_secs(45);
const MAX_APPLICATIONS: usize = 5_000;
const MAX_UPDATE_HISTORY: usize = 1_000;
const MAX_LAUNCHD_ITEMS: usize = 1_000;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct MacosNetworkHardwarePort {
    hardware_port: String,
    device: String,
    ethernet_address: String,
}

#[derive(Debug, Clone, Default)]
struct SwVers {
    product_name: String,
    product_version: String,
    build_version: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct MacosHardwareOverview {
    model_name: Option<String>,
    model_identifier: Option<String>,
    chip: Option<String>,
    processor_name: Option<String>,
    processor_speed: Option<String>,
    memory: Option<String>,
    serial_number: Option<String>,
    hardware_uuid: Option<String>,
    provisioning_udid: Option<String>,
    activation_lock_status: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct MacosProxySettings {
    enabled: bool,
    auto_detect: bool,
    proxy_server: Option<String>,
    bypass_list: Vec<String>,
    pac_url: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct MacosBatteryInfo {
    present: bool,
    percentage: Option<u64>,
    is_charging: Option<bool>,
    fully_charged: Option<bool>,
    cycle_count: Option<u64>,
    condition: Option<String>,
    health: Option<String>,
    ac_power_connected: Option<bool>,
    time_remaining: Option<String>,
}

pub(crate) async fn collect_snapshot(
    agent_id: &str,
    hostname: &str,
    boot_session_id: &str,
    agent_version: &str,
) -> Result<(String, Value)> {
    let collected_at = Utc::now().to_rfc3339();

    let mut sys = System::new_all();
    let mut disks = Disks::new_with_refreshed_list();
    let mut networks = Networks::new_with_refreshed_list();
    let inventory = super::collect_inventory(&mut sys, &mut disks, &mut networks);
    let collection = serde_json::to_value(&inventory).context("serialize macOS inventory")?;

    let sw_vers = collect_sw_vers().await;
    let hardware = collect_hardware_overview().await;
    let installed_programs = collect_installed_programs().await;
    let gpus = collect_gpus().await;
    let network_hardware_ports = collect_network_hardware_ports().await;
    let proxy_settings = collect_proxy_settings().await;
    let battery = collect_battery_info().await;
    let (services, startup_items) = collect_services_and_startup_items().await;
    let pending_updates = collect_pending_updates().await;
    let update_history = collect_update_history().await;
    let automatic_updates_enabled = collect_automatic_update_status().await.unwrap_or(false);

    let app_count = installed_programs.len();
    let service_count = services.len();
    let startup_item_count = startup_items.len();
    let pending_update_count = pending_updates.len();
    let collection = build_normalized_collection(
        collection,
        sys.physical_core_count(),
        &sw_vers,
        &hardware,
        installed_programs,
        gpus,
        network_hardware_ports,
        proxy_settings,
        battery,
        services,
        startup_items,
        pending_updates,
        update_history,
        automatic_updates_enabled,
    )?;

    let snapshot = json!({
        "metadata": {
            "agent_id": agent_id,
            "device_name": hostname,
            "boot_session_id": boot_session_id,
            "agent_version": agent_version,
            "collection_profile": "macos_full",
            "timestamp": collected_at,
        },
        "collection": collection,
    });

    info!(
        app_count,
        service_count, startup_item_count, pending_update_count, "macOS full snapshot collected"
    );

    Ok((collected_at, snapshot))
}

fn build_normalized_collection(
    mut collection: Value,
    physical_core_count: Option<usize>,
    sw_vers: &SwVers,
    hardware: &MacosHardwareOverview,
    installed_programs: Vec<Value>,
    gpus: Vec<Value>,
    network_hardware_ports: HashMap<String, MacosNetworkHardwarePort>,
    proxy_settings: MacosProxySettings,
    battery: Option<MacosBatteryInfo>,
    services: Vec<Value>,
    startup_items: Vec<Value>,
    pending_updates: Vec<Value>,
    update_history: Vec<Value>,
    automatic_updates_enabled: bool,
) -> Result<Value> {
    let collection_obj = collection
        .as_object_mut()
        .context("macOS inventory did not serialize to an object")?;

    let system = as_object_value(collection_obj, "system");
    let cpu = as_object_value(collection_obj, "cpu");
    let memory = as_object_value(collection_obj, "memory");
    let disks_value = collection_obj
        .get("disks")
        .cloned()
        .unwrap_or_else(|| Value::Array(Vec::new()));
    let networks_value = collection_obj
        .get("networks")
        .cloned()
        .unwrap_or_else(|| Value::Array(Vec::new()));
    let reboot_required = macos_reboot_required(&pending_updates);

    collection_obj.insert(
        "operating_system".to_string(),
        json!({
            "system": build_system_info(&system, sw_vers, hardware),
            "services": build_services_info(&services),
            "updates": build_updates_info(
                &pending_updates,
                &update_history,
                reboot_required,
                automatic_updates_enabled,
            ),
        }),
    );
    collection_obj.insert(
        "hardware".to_string(),
        json!({
            "cpu": build_cpu_info(&cpu, physical_core_count),
            "memory": build_memory_info(&memory),
            "disks": build_hardware_disks(&disks_value),
            "gpus": gpus,
            "network_adapters": build_hardware_network_adapters(&networks_value, &network_hardware_ports),
            "tpm": null,
            "secure_boot": null,
            "battery": build_battery_info(battery.as_ref()),
            "motherboard": build_motherboard_info(hardware),
            "todo_data_collection": [],
        }),
    );
    collection_obj.insert(
        "network".to_string(),
        json!({
            "adapters": build_network_adapters(&networks_value, &network_hardware_ports),
            "routing_table": [],
            "dns_cache_entries": 0,
            "active_connections": {
                "tcp_established": 0,
                "tcp_time_wait": 0,
                "tcp_close_wait": 0,
                "tcp_other": 0,
                "udp_listeners": 0,
            },
            "shares": [],
            "proxy": build_proxy_info(&proxy_settings),
            "firewall_rules_count": null,
            "todo_data_collection": [],
        }),
    );
    collection_obj.insert(
        "software".to_string(),
        json!({
            "installed_programs": installed_programs,
            "software_updates": build_software_update_summary(
                &pending_updates,
                &update_history,
                reboot_required,
                automatic_updates_enabled,
            ),
            "macos_updates": build_software_update_summary(
                &pending_updates,
                &update_history,
                reboot_required,
                automatic_updates_enabled,
            ),
            "windows_updates": build_windows_update_summary(
                &pending_updates,
                &update_history,
                reboot_required,
                automatic_updates_enabled,
            ),
            "features": [],
            "startup_items": startup_items,
            "dot_net_versions": [],
            "todo_data_collection": [],
        }),
    );
    collection_obj.insert(
        "unsupported_features".to_string(),
        json!({
            "remote_registry": "unsupported_platform",
            "chat": "unsupported_platform",
        }),
    );

    Ok(collection)
}

async fn collect_sw_vers() -> SwVers {
    let Some(stdout) = run_command("sw_vers", &[]).await else {
        return SwVers::default();
    };
    parse_sw_vers_output(&stdout)
}

fn parse_sw_vers_output(stdout: &str) -> SwVers {
    let mut sw_vers = SwVers::default();
    for line in stdout.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim().to_string();
        match key.trim() {
            "ProductName" => sw_vers.product_name = value,
            "ProductVersion" => sw_vers.product_version = value,
            "BuildVersion" => sw_vers.build_version = value,
            _ => {}
        }
    }
    sw_vers
}

async fn collect_hardware_overview() -> MacosHardwareOverview {
    let Some(stdout) = run_command_with_timeout(
        "system_profiler",
        &["SPHardwareDataType", "-json"],
        SYSTEM_PROFILER_TIMEOUT,
    )
    .await
    else {
        return MacosHardwareOverview::default();
    };
    parse_system_profiler_hardware(&stdout)
}

fn parse_system_profiler_hardware(stdout: &str) -> MacosHardwareOverview {
    let Ok(value) = serde_json::from_str::<Value>(stdout) else {
        return MacosHardwareOverview::default();
    };
    let Some(record) = value
        .get("SPHardwareDataType")
        .and_then(Value::as_array)
        .and_then(|items| items.first())
        .and_then(Value::as_object)
    else {
        return MacosHardwareOverview::default();
    };

    MacosHardwareOverview {
        model_name: text_value(record.get("machine_name")),
        model_identifier: text_value(record.get("machine_model")),
        chip: text_value(record.get("chip_type")),
        processor_name: text_value(record.get("cpu_type")),
        processor_speed: text_value(record.get("current_processor_speed")),
        memory: text_value(record.get("physical_memory")),
        serial_number: text_value(record.get("serial_number")),
        hardware_uuid: text_value(record.get("platform_UUID")),
        provisioning_udid: text_value(record.get("provisioning_UDID")),
        activation_lock_status: text_value(record.get("activation_lock_status")),
    }
}

async fn collect_battery_info() -> Option<MacosBatteryInfo> {
    let stdout = run_command_with_timeout(
        "system_profiler",
        &["SPPowerDataType", "-json"],
        SYSTEM_PROFILER_TIMEOUT,
    )
    .await?;
    parse_system_profiler_power(&stdout)
}

fn parse_system_profiler_power(stdout: &str) -> Option<MacosBatteryInfo> {
    let value = serde_json::from_str::<Value>(stdout).ok()?;
    let power = value
        .get("SPPowerDataType")
        .and_then(Value::as_array)?
        .first()?;

    let percentage = find_text_by_key(power, "sppower_battery_state_of_charge")
        .and_then(|value| parse_u64_prefix(&value));
    let cycle_count = find_text_by_key(power, "sppower_battery_cycle_count")
        .and_then(|value| parse_u64_prefix(&value));
    let condition = find_text_by_key(power, "sppower_battery_condition");
    let health = find_text_by_key(power, "sppower_battery_health");
    let is_charging = find_text_by_key(power, "sppower_battery_is_charging")
        .and_then(|value| parse_bool_text(&value));
    let fully_charged = find_text_by_key(power, "sppower_battery_fully_charged")
        .and_then(|value| parse_bool_text(&value));
    let ac_power_connected = find_text_by_key(power, "sppower_ac_charger_connected")
        .and_then(|value| parse_bool_text(&value));
    let time_remaining = find_text_by_key(power, "sppower_battery_time_remaining");

    let present = percentage.is_some()
        || cycle_count.is_some()
        || condition.is_some()
        || health.is_some()
        || is_charging.is_some()
        || fully_charged.is_some();
    if !present {
        return None;
    }

    Some(MacosBatteryInfo {
        present: true,
        percentage,
        is_charging,
        fully_charged,
        cycle_count,
        condition,
        health,
        ac_power_connected,
        time_remaining,
    })
}

pub(crate) async fn collect_installed_programs() -> Vec<Value> {
    if let Some(stdout) = run_command_with_timeout(
        "system_profiler",
        &["SPApplicationsDataType", "-json"],
        SYSTEM_PROFILER_TIMEOUT,
    )
    .await
    {
        let programs = parse_system_profiler_applications(&stdout);
        if !programs.is_empty() {
            return programs;
        }
    }

    scan_application_dirs()
}

fn parse_system_profiler_applications(stdout: &str) -> Vec<Value> {
    let Ok(value) = serde_json::from_str::<Value>(stdout) else {
        return Vec::new();
    };
    let Some(apps) = value
        .get("SPApplicationsDataType")
        .and_then(Value::as_array)
    else {
        return Vec::new();
    };

    let mut dedupe = HashSet::new();
    let mut programs = Vec::new();
    for app in apps {
        let Some(record) = app.as_object() else {
            continue;
        };
        let Some(name) = text_value(record.get("_name").or_else(|| record.get("name"))) else {
            continue;
        };
        let version = text_value(record.get("version"));
        let location = text_value(record.get("path"));
        let key = format!(
            "{}|{}|{}",
            normalize_text(&name),
            version.clone().unwrap_or_default(),
            location.clone().unwrap_or_default()
        );
        if !dedupe.insert(key) {
            continue;
        }
        let publisher = text_value(record.get("obtained_from"))
            .or_else(|| first_string_in_array(record.get("signed_by")));
        let size_bytes = text_value(record.get("size")).and_then(|size| parse_size_text(&size));
        let architecture = text_value(record.get("arch_kind"));
        let install_date = text_value(record.get("lastModified"))
            .or_else(|| text_value(record.get("last_modified")));
        let is_64_bit = architecture
            .as_deref()
            .map(|value| value.contains("64") || value.eq_ignore_ascii_case("universal"));

        programs.push(json!({
            "name": name,
            "version": version,
            "publisher": publisher,
            "vendor": publisher,
            "install_date": install_date,
            "size_bytes": size_bytes,
            "source": "system_profiler",
            "location": location,
            "architecture": architecture,
            "is_64_bit": is_64_bit,
        }));
        if programs.len() >= MAX_APPLICATIONS {
            break;
        }
    }

    programs
}

fn scan_application_dirs() -> Vec<Value> {
    let mut programs = Vec::new();
    let mut seen = HashSet::new();
    for root in ["/Applications", "/System/Applications"] {
        let Ok(entries) = fs::read_dir(root) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|value| value.to_str()) != Some("app") {
                continue;
            }
            let Some(file_name) = path.file_name().and_then(|value| value.to_str()) else {
                continue;
            };
            let name = file_name.trim_end_matches(".app").to_string();
            let key = normalize_text(&name);
            if key.is_empty() || !seen.insert(key) {
                continue;
            }
            programs.push(json!({
                "name": name,
                "version": null,
                "publisher": null,
                "vendor": null,
                "install_date": null,
                "size_bytes": null,
                "source": "application_bundle",
                "location": path.to_string_lossy().to_string(),
                "architecture": null,
                "is_64_bit": null,
            }));
            if programs.len() >= MAX_APPLICATIONS {
                return programs;
            }
        }
    }
    programs
}

async fn collect_gpus() -> Vec<Value> {
    let Some(stdout) = run_command_with_timeout(
        "system_profiler",
        &["SPDisplaysDataType", "-json"],
        SYSTEM_PROFILER_TIMEOUT,
    )
    .await
    else {
        return Vec::new();
    };
    parse_system_profiler_displays(&stdout)
}

fn parse_system_profiler_displays(stdout: &str) -> Vec<Value> {
    let Ok(value) = serde_json::from_str::<Value>(stdout) else {
        return Vec::new();
    };
    let Some(displays) = value.get("SPDisplaysDataType").and_then(Value::as_array) else {
        return Vec::new();
    };

    displays
        .iter()
        .filter_map(|display| {
            let record = display.as_object()?;
            let name = text_value(record.get("sppci_model"))
                .or_else(|| text_value(record.get("_name").or_else(|| record.get("name"))))?;
            let vendor = text_value(record.get("spdisplays_vendor"))
                .or_else(|| infer_gpu_vendor(&name))
                .unwrap_or_default();
            let vram = text_value(record.get("spdisplays_vram"));
            let displays = record
                .get("spdisplays_ndrvs")
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(normalize_attached_display)
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();

            Some(json!({
                "name": name,
                "model": name,
                "vendor": vendor,
                "manufacturer": vendor,
                "driver_version": text_value(record.get("spdisplays_rom-revision")),
                "device_id": text_value(record.get("spdisplays_device-id")),
                "revision_id": text_value(record.get("spdisplays_revision-id")),
                "vram": vram,
                "vram_bytes": vram.as_deref().and_then(parse_size_text),
                "metal": text_value(record.get("spdisplays_metal")),
                "displays": displays,
                "source": "system_profiler",
            }))
        })
        .collect()
}

fn normalize_attached_display(value: &Value) -> Option<Value> {
    let record = value.as_object()?;
    let name = text_value(record.get("_name").or_else(|| record.get("name")))?;
    Some(json!({
        "name": name,
        "resolution": text_value(record.get("_spdisplays_resolution").or_else(|| record.get("spdisplays_resolution"))),
        "main": text_value(record.get("spdisplays_main")).and_then(|value| parse_macos_bool(&value)),
        "mirror": text_value(record.get("spdisplays_mirror")).and_then(|value| parse_macos_bool(&value)),
        "online": text_value(record.get("spdisplays_online")).and_then(|value| parse_macos_bool(&value)),
        "built_in": text_value(record.get("spdisplays_built-in")).and_then(|value| parse_macos_bool(&value)),
    }))
}

fn parse_macos_bool(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "yes" | "true" | "spdisplays_yes" => Some(true),
        "no" | "false" | "spdisplays_no" => Some(false),
        _ => None,
    }
}

fn infer_gpu_vendor(name: &str) -> Option<String> {
    let normalized = name.to_ascii_lowercase();
    if normalized.contains("apple") {
        Some("Apple".to_string())
    } else if normalized.contains("amd") || normalized.contains("radeon") {
        Some("AMD".to_string())
    } else if normalized.contains("intel") {
        Some("Intel".to_string())
    } else if normalized.contains("nvidia") || normalized.contains("geforce") {
        Some("NVIDIA".to_string())
    } else {
        None
    }
}

async fn collect_network_hardware_ports() -> HashMap<String, MacosNetworkHardwarePort> {
    let Some(stdout) = run_command("networksetup", &["-listallhardwareports"]).await else {
        return HashMap::new();
    };
    parse_network_hardware_ports(&stdout)
}

fn parse_network_hardware_ports(stdout: &str) -> HashMap<String, MacosNetworkHardwarePort> {
    let mut ports = HashMap::new();
    let mut current = MacosNetworkHardwarePort::default();

    for raw_line in stdout.lines() {
        let line = raw_line.trim();
        if line.is_empty() {
            flush_network_hardware_port(&mut ports, &mut current);
            continue;
        }
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim().to_string();
        match key.trim() {
            "Hardware Port" => {
                flush_network_hardware_port(&mut ports, &mut current);
                current.hardware_port = value;
            }
            "Device" => current.device = value,
            "Ethernet Address" => current.ethernet_address = value,
            _ => {}
        }
    }

    flush_network_hardware_port(&mut ports, &mut current);
    ports
}

fn flush_network_hardware_port(
    ports: &mut HashMap<String, MacosNetworkHardwarePort>,
    current: &mut MacosNetworkHardwarePort,
) {
    if !current.device.trim().is_empty() {
        ports.insert(current.device.clone(), current.clone());
    }
    *current = MacosNetworkHardwarePort::default();
}

async fn collect_proxy_settings() -> MacosProxySettings {
    let Some(stdout) = run_command("scutil", &["--proxy"]).await else {
        return MacosProxySettings::default();
    };
    parse_scutil_proxy_settings(&stdout)
}

fn parse_scutil_proxy_settings(stdout: &str) -> MacosProxySettings {
    let mut fields = HashMap::new();
    let mut exceptions = Vec::new();
    let mut in_exceptions = false;

    for raw_line in stdout.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line == "<dictionary> {" || line == "}" {
            in_exceptions = false;
            continue;
        }
        if line.starts_with("ExceptionsList") {
            in_exceptions = true;
            continue;
        }
        if in_exceptions {
            if let Some((_, value)) = line.split_once(':') {
                let value = value.trim();
                if !value.is_empty() {
                    exceptions.push(value.to_string());
                }
            }
            continue;
        }
        if let Some((key, value)) = line.split_once(':') {
            fields.insert(key.trim().to_string(), value.trim().to_string());
        }
    }

    let http_enabled = parse_numeric_bool(fields.get("HTTPEnable"));
    let https_enabled = parse_numeric_bool(fields.get("HTTPSEnable"));
    let socks_enabled = parse_numeric_bool(fields.get("SOCKSEnable"));
    let ftp_enabled = parse_numeric_bool(fields.get("FTPEnable"));
    let auto_detect = parse_numeric_bool(fields.get("ProxyAutoDiscoveryEnable"));
    let pac_url = if parse_numeric_bool(fields.get("ProxyAutoConfigEnable")) {
        fields
            .get("ProxyAutoConfigURLString")
            .cloned()
            .filter(|value| !value.is_empty())
    } else {
        None
    };
    let proxy_server = [
        ("https", "HTTPSProxy", "HTTPSPort", https_enabled),
        ("http", "HTTPProxy", "HTTPPort", http_enabled),
        ("socks", "SOCKSProxy", "SOCKSPort", socks_enabled),
        ("ftp", "FTPProxy", "FTPPort", ftp_enabled),
    ]
    .iter()
    .find_map(|(scheme, host_key, port_key, enabled)| {
        if !enabled {
            return None;
        }
        let host = fields.get(*host_key)?.trim();
        if host.is_empty() {
            return None;
        }
        let port = fields
            .get(*port_key)
            .map(String::as_str)
            .unwrap_or("")
            .trim();
        if port.is_empty() {
            Some(format!("{scheme}://{host}"))
        } else {
            Some(format!("{scheme}://{host}:{port}"))
        }
    });

    MacosProxySettings {
        enabled: http_enabled || https_enabled || socks_enabled || ftp_enabled || pac_url.is_some(),
        auto_detect,
        proxy_server,
        bypass_list: exceptions,
        pac_url,
    }
}

fn parse_numeric_bool(value: Option<&String>) -> bool {
    value
        .map(|value| matches!(value.trim(), "1" | "true" | "TRUE" | "yes" | "YES"))
        .unwrap_or(false)
}

pub(crate) async fn collect_services_and_startup_items() -> (Vec<Value>, Vec<Value>) {
    let startup_items = collect_launchd_startup_items().await;
    let mut startup_by_label = HashMap::new();
    for item in &startup_items {
        if let Some(label) = item.get("label").and_then(Value::as_str) {
            startup_by_label.insert(label.to_string(), item.clone());
        }
    }

    let launchctl_stdout = run_command("launchctl", &["list"])
        .await
        .unwrap_or_default();
    let mut services = parse_launchctl_list_output(&launchctl_stdout);
    let mut service_labels = HashSet::new();
    for service in &mut services {
        let Some(service_obj) = service.as_object_mut() else {
            continue;
        };
        let Some(label) = service_obj
            .get("name")
            .and_then(Value::as_str)
            .map(str::to_string)
        else {
            continue;
        };
        service_labels.insert(label.clone());
        if let Some(startup) = startup_by_label.get(&label).and_then(Value::as_object) {
            if let Some(location) = startup.get("location").cloned() {
                service_obj.insert("path".to_string(), location);
            }
            if let Some(command) = startup.get("command").cloned() {
                service_obj.insert("binary_path".to_string(), command);
            }
            if let Some(enabled) = startup.get("is_enabled").cloned() {
                service_obj.insert("is_enabled".to_string(), enabled);
            }
        }
    }

    for item in &startup_items {
        let Some(label) = item.get("label").and_then(Value::as_str) else {
            continue;
        };
        if service_labels.contains(label) {
            continue;
        }
        services.push(json!({
            "name": label,
            "service_name": label,
            "display_name": label,
            "status": "stopped",
            "start_type": "launchd",
            "account": item.get("user").cloned().unwrap_or(Value::Null),
            "process_id": null,
            "can_stop": null,
            "can_pause": false,
            "is_critical": false,
            "description": null,
            "path": item.get("location").cloned().unwrap_or(Value::Null),
            "binary_path": item.get("command").cloned().unwrap_or(Value::Null),
            "is_enabled": item.get("is_enabled").cloned().unwrap_or(Value::Null),
        }));
    }

    (services, startup_items)
}

fn parse_launchctl_list_output(stdout: &str) -> Vec<Value> {
    let mut services = Vec::new();
    for line in stdout.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with("PID") {
            continue;
        }
        let columns = trimmed.split_whitespace().collect::<Vec<_>>();
        if columns.len() < 3 {
            continue;
        }
        let pid = if columns[0] == "-" {
            None
        } else {
            columns[0].parse::<u32>().ok()
        };
        let exit_status = columns[1].parse::<i32>().ok();
        let label = columns[2..].join(" ");
        if label.trim().is_empty() {
            continue;
        }
        let status = match (pid, exit_status) {
            (Some(_), _) => "running".to_string(),
            (None, Some(0)) => "stopped".to_string(),
            (None, Some(code)) => format!("exited({code})"),
            (None, None) => "unknown".to_string(),
        };
        services.push(json!({
            "name": label,
            "service_name": label,
            "display_name": label,
            "status": status,
            "start_type": "launchd",
            "account": null,
            "process_id": pid,
            "can_stop": null,
            "can_pause": false,
            "is_critical": false,
            "description": null,
            "path": null,
        }));
    }
    services
}

async fn collect_launchd_startup_items() -> Vec<Value> {
    let mut items = Vec::new();
    for (path, user) in launchd_dirs() {
        collect_launchd_startup_items_from_dir(&path, user, &mut items).await;
        if items.len() >= MAX_LAUNCHD_ITEMS {
            break;
        }
    }
    items
}

async fn collect_launchd_startup_items_from_dir(path: &Path, user: &str, out: &mut Vec<Value>) {
    let Ok(entries) = fs::read_dir(path) else {
        return;
    };
    for entry in entries.flatten() {
        if out.len() >= MAX_LAUNCHD_ITEMS {
            return;
        }
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) != Some("plist") {
            continue;
        }
        let Some(path_str) = path.to_str() else {
            continue;
        };
        let Some(stdout) = run_command_with_timeout(
            "plutil",
            &["-convert", "json", "-o", "-", path_str],
            PLUTIL_TIMEOUT,
        )
        .await
        else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<Value>(&stdout) else {
            continue;
        };
        if let Some(item) = startup_item_from_plist_json(&value, &path, user) {
            out.push(item);
        }
    }
}

fn launchd_dirs() -> Vec<(PathBuf, &'static str)> {
    vec![
        (PathBuf::from("/Library/LaunchDaemons"), "root"),
        (PathBuf::from("/Library/LaunchAgents"), "user"),
        (PathBuf::from("/System/Library/LaunchDaemons"), "root"),
        (PathBuf::from("/System/Library/LaunchAgents"), "user"),
    ]
}

fn startup_item_from_plist_json(value: &Value, path: &Path, user: &str) -> Option<Value> {
    let record = value.as_object()?;
    let label = text_value(record.get("Label")).or_else(|| {
        path.file_stem()
            .and_then(|value| value.to_str())
            .map(str::to_string)
    })?;
    let command = text_value(record.get("Program"))
        .or_else(|| program_arguments_to_command(record.get("ProgramArguments")));
    let disabled = record
        .get("Disabled")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let location = path.to_string_lossy().to_string();

    Some(json!({
        "name": label,
        "label": label,
        "item_name": label,
        "command": command.clone().unwrap_or_else(|| label.clone()),
        "location": location,
        "source": location,
        "user": user,
        "is_enabled": !disabled,
    }))
}

fn program_arguments_to_command(value: Option<&Value>) -> Option<String> {
    let args = value?.as_array()?;
    let mut parts = Vec::new();
    for arg in args {
        let Some(text) = arg.as_str() else {
            continue;
        };
        if text.contains(char::is_whitespace) {
            parts.push(format!("\"{}\"", text.replace('"', "\\\"")));
        } else {
            parts.push(text.to_string());
        }
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join(" "))
    }
}

pub(crate) async fn collect_pending_updates() -> Vec<Value> {
    let Some(stdout) =
        run_command_with_timeout("softwareupdate", &["-l"], SOFTWAREUPDATE_TIMEOUT).await
    else {
        debug!("softwareupdate -l returned no output");
        return Vec::new();
    };
    parse_softwareupdate_list_output(&stdout)
}

fn parse_softwareupdate_list_output(stdout: &str) -> Vec<Value> {
    let mut updates = Vec::new();
    let mut current_label: Option<String> = None;
    let mut current_fields: Map<String, Value> = Map::new();

    for raw_line in stdout.lines() {
        let line = raw_line.trim();
        if line.is_empty()
            || line
                .to_ascii_lowercase()
                .starts_with("software update tool")
        {
            continue;
        }
        if line.eq_ignore_ascii_case("No new software available.") {
            current_label = None;
            current_fields.clear();
            break;
        }
        if let Some(label) = parse_softwareupdate_label_line(line) {
            flush_software_update(&mut updates, &mut current_label, &mut current_fields);
            current_label = Some(label.trim_end_matches(',').to_string());
            continue;
        }
        let lower_line = line.to_ascii_lowercase();
        if lower_line.starts_with("title:")
            || lower_line.contains("action:")
            || lower_line.contains("recommended:")
            || lower_line.contains("size:")
        {
            for (key, value) in parse_colon_fields(line) {
                current_fields.insert(key, Value::String(value));
            }
        }
    }

    flush_software_update(&mut updates, &mut current_label, &mut current_fields);
    filter_latest_macos_os_updates(updates)
}

fn filter_latest_macos_os_updates(updates: Vec<Value>) -> Vec<Value> {
    let latest = updates
        .iter()
        .filter_map(|update| {
            update
                .get("title")
                .and_then(Value::as_str)
                .and_then(crate::patching::macos_os_update_version_parts)
        })
        .max_by(|left, right| crate::patching::compare_patch_version_parts(left, right));
    let Some(latest) = latest else {
        return updates;
    };

    updates
        .into_iter()
        .filter(|update| {
            update
                .get("title")
                .and_then(Value::as_str)
                .and_then(crate::patching::macos_os_update_version_parts)
                .map(|version| {
                    crate::patching::compare_patch_version_parts(&version, &latest)
                        == std::cmp::Ordering::Equal
                })
                .unwrap_or(true)
        })
        .collect()
}

fn parse_softwareupdate_label_line(line: &str) -> Option<&str> {
    let trimmed = line.trim();
    let candidate = trimmed.strip_prefix('*').map(str::trim).unwrap_or(trimmed);
    let colon = candidate.find(':')?;
    let key = candidate[..colon].trim();
    if !key.eq_ignore_ascii_case("label") {
        return None;
    }
    let value = candidate[colon + 1..].trim().trim_end_matches(',').trim();
    if value.is_empty() {
        None
    } else {
        Some(value)
    }
}

pub(crate) async fn collect_update_history() -> Vec<Value> {
    let Some(stdout) = run_command_with_timeout(
        "system_profiler",
        &["SPInstallHistoryDataType", "-json"],
        SYSTEM_PROFILER_TIMEOUT,
    )
    .await
    else {
        return Vec::new();
    };
    parse_system_profiler_install_history(&stdout)
}

fn parse_system_profiler_install_history(stdout: &str) -> Vec<Value> {
    let Ok(value) = serde_json::from_str::<Value>(stdout) else {
        return Vec::new();
    };
    let Some(items) = value
        .get("SPInstallHistoryDataType")
        .and_then(Value::as_array)
    else {
        return Vec::new();
    };

    let mut history = Vec::new();
    let mut dedupe = HashSet::new();
    for item in items {
        let Some(record) = item.as_object() else {
            continue;
        };
        let Some(title) = text_value(
            record
                .get("_name")
                .or_else(|| record.get("name"))
                .or_else(|| record.get("title")),
        ) else {
            continue;
        };
        let version = text_value(
            record
                .get("install_version")
                .or_else(|| record.get("version"))
                .or_else(|| record.get("package_version")),
        );
        let installed_at = text_value(
            record
                .get("install_date")
                .or_else(|| record.get("date"))
                .or_else(|| record.get("installed_at")),
        )
        .and_then(|value| normalize_macos_install_date(&value))
        .or_else(|| {
            text_value(
                record
                    .get("install_date")
                    .or_else(|| record.get("date"))
                    .or_else(|| record.get("installed_at")),
            )
        });
        let source = text_value(
            record
                .get("package_source")
                .or_else(|| record.get("install_source"))
                .or_else(|| record.get("source")),
        )
        .unwrap_or_else(|| "system_profiler".to_string());
        let category = text_value(
            record
                .get("install_type")
                .or_else(|| record.get("content_download_type"))
                .or_else(|| record.get("category")),
        );
        let update_key = crate::patching::build_patch_update_key(&title, None);
        let dedupe_key = format!(
            "{}|{}|{}",
            normalize_text(&title),
            version.clone().unwrap_or_default(),
            installed_at.clone().unwrap_or_default()
        );
        if !dedupe.insert(dedupe_key) {
            continue;
        }

        history.push(json!({
            "title": title,
            "name": title.clone(),
            "version": version,
            "installed_at": installed_at.clone(),
            "installedAt": installed_at.clone(),
            "date": installed_at.clone(),
            "operation": "install",
            "result": "succeeded",
            "status": "succeeded",
            "kb_article": null,
            "kbArticle": null,
            "update_key": update_key,
            "updateKey": update_key,
            "source": source,
            "category": category,
        }));
        if history.len() >= MAX_UPDATE_HISTORY {
            break;
        }
    }

    history.sort_by(|a, b| {
        let a_date = a.get("installed_at").and_then(Value::as_str).unwrap_or("");
        let b_date = b.get("installed_at").and_then(Value::as_str).unwrap_or("");
        b_date.cmp(a_date)
    });
    history
}

fn normalize_macos_install_date(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Ok(parsed) = DateTime::parse_from_rfc3339(trimmed) {
        return Some(parsed.with_timezone(&Utc).to_rfc3339());
    }
    DateTime::parse_from_str(trimmed, "%Y-%m-%d %H:%M:%S %z")
        .ok()
        .map(|parsed| parsed.with_timezone(&Utc).to_rfc3339())
}

fn flush_software_update(
    updates: &mut Vec<Value>,
    current_label: &mut Option<String>,
    current_fields: &mut Map<String, Value>,
) {
    let Some(label) = current_label.take() else {
        current_fields.clear();
        return;
    };
    let title = current_fields
        .get("title")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| label.clone());
    let version = current_fields
        .get("version")
        .and_then(Value::as_str)
        .map(str::to_string);
    let size_bytes = current_fields
        .get("size")
        .and_then(Value::as_str)
        .and_then(parse_size_text);
    let recommended = current_fields
        .get("recommended")
        .and_then(Value::as_str)
        .map(|value| value.eq_ignore_ascii_case("yes") || value.eq_ignore_ascii_case("true"))
        .unwrap_or(false);
    let requires_reboot = current_fields
        .get("action")
        .and_then(Value::as_str)
        .map(|value| value.to_ascii_lowercase().contains("restart"))
        .unwrap_or(false);
    let update_key = crate::patching::build_patch_update_key(&title, None);

    updates.push(json!({
        "title": title,
        "name": title,
        "description": label,
        "label": label,
        "version": version,
        "kb_article": null,
        "kbArticle": null,
        "update_key": update_key,
        "updateKey": update_key,
        "size_bytes": size_bytes,
        "sizeBytes": size_bytes,
        "requires_reboot": requires_reboot,
        "requiresReboot": requires_reboot,
        "is_mandatory": recommended,
        "isMandatory": recommended,
        "recommended": recommended,
        "source": "softwareupdate",
    }));
    current_fields.clear();
}

fn parse_colon_fields(line: &str) -> Vec<(String, String)> {
    parse_known_colon_fields(line, &["Title", "Version", "Size", "Recommended", "Action"])
}

fn parse_known_colon_fields(line: &str, keys: &[&str]) -> Vec<(String, String)> {
    let mut markers = keys
        .iter()
        .filter_map(|key| {
            let marker = format!("{key}:");
            find_ascii_case_insensitive(line, &marker).map(|offset| (offset, *key, marker.len()))
        })
        .collect::<Vec<_>>();
    markers.sort_by_key(|(offset, _, _)| *offset);

    markers
        .iter()
        .enumerate()
        .filter_map(|(index, (offset, key, marker_len))| {
            let value_start = offset + marker_len;
            let value_end = markers
                .get(index + 1)
                .map(|(next_offset, _, _)| *next_offset)
                .unwrap_or(line.len());
            let value = line[value_start..value_end]
                .trim()
                .trim_start_matches(',')
                .trim()
                .trim_end_matches(',')
                .trim()
                .to_string();
            if value.is_empty() {
                None
            } else {
                Some((key.to_ascii_lowercase().replace(' ', "_"), value))
            }
        })
        .collect()
}

fn find_ascii_case_insensitive(haystack: &str, needle: &str) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack
        .as_bytes()
        .windows(needle.len())
        .position(|window| window.eq_ignore_ascii_case(needle.as_bytes()))
}

async fn collect_automatic_update_status() -> Option<bool> {
    let stdout = run_command("softwareupdate", &["--schedule"]).await?;
    parse_softwareupdate_schedule_output(&stdout)
}

fn parse_softwareupdate_schedule_output(stdout: &str) -> Option<bool> {
    let normalized = stdout.trim().to_ascii_lowercase();
    if normalized.contains("automatic check is on") {
        Some(true)
    } else if normalized.contains("automatic check is off") {
        Some(false)
    } else {
        None
    }
}

fn build_system_info(
    system: &Map<String, Value>,
    sw_vers: &SwVers,
    hardware: &MacosHardwareOverview,
) -> Value {
    let hostname = read_string(system, "hostname").unwrap_or_else(|| "unknown".to_string());
    let product_name = if sw_vers.product_name.trim().is_empty() {
        read_string(system, "os_name").unwrap_or_else(|| "macOS".to_string())
    } else {
        sw_vers.product_name.clone()
    };
    let product_version = if sw_vers.product_version.trim().is_empty() {
        read_string(system, "os_version").unwrap_or_default()
    } else {
        sw_vers.product_version.clone()
    };
    let build = if sw_vers.build_version.trim().is_empty() {
        read_string(system, "kernel_version").unwrap_or_default()
    } else {
        sw_vers.build_version.clone()
    };
    let architecture =
        read_string(system, "architecture").unwrap_or_else(|| std::env::consts::ARCH.to_string());
    let uptime_seconds = read_u64(system, "uptime_seconds").unwrap_or(0);
    let boot_time = read_u64(system, "boot_time");

    json!({
        "hostname": hostname,
        "domain": null,
        "name": product_name,
        "version": product_version,
        "os": {
            "name": product_name,
            "version": product_version,
            "build": build,
            "edition": "",
            "install_date": null,
            "architecture": architecture,
            "locale": std::env::var("LANG").unwrap_or_default(),
            "timezone": local_timezone_name(),
            "serial_number": hardware.serial_number.clone(),
            "model": hardware.model_name.clone(),
            "model_identifier": hardware.model_identifier.clone(),
            "hardware_uuid": hardware.hardware_uuid.clone(),
            "provisioning_udid": hardware.provisioning_udid.clone(),
            "activation_lock_status": hardware.activation_lock_status.clone(),
        },
        "boot_time": boot_time.and_then(epoch_to_rfc3339),
        "uptime_seconds": uptime_seconds,
        "todo_data_collection": [],
    })
}

fn build_motherboard_info(hardware: &MacosHardwareOverview) -> Value {
    if hardware.model_name.is_none()
        && hardware.model_identifier.is_none()
        && hardware.serial_number.is_none()
        && hardware.hardware_uuid.is_none()
    {
        return Value::Null;
    }

    json!({
        "manufacturer": "Apple",
        "product": hardware.model_identifier.clone(),
        "model": hardware.model_name.clone(),
        "serial_number": hardware.serial_number.clone(),
        "uuid": hardware.hardware_uuid.clone(),
        "chip": hardware.chip.clone(),
        "processor_name": hardware.processor_name.clone(),
        "processor_speed": hardware.processor_speed.clone(),
        "memory": hardware.memory.clone(),
    })
}

fn build_battery_info(battery: Option<&MacosBatteryInfo>) -> Value {
    let Some(battery) = battery else {
        return Value::Null;
    };

    json!({
        "present": battery.present,
        "percentage": battery.percentage,
        "is_charging": battery.is_charging,
        "fully_charged": battery.fully_charged,
        "cycle_count": battery.cycle_count,
        "condition": battery.condition,
        "health": battery.health,
        "ac_power_connected": battery.ac_power_connected,
        "time_remaining": battery.time_remaining,
        "source": "system_profiler",
    })
}

fn build_cpu_info(cpu: &Map<String, Value>, physical_core_count: Option<usize>) -> Value {
    let brand = read_string(cpu, "brand").unwrap_or_default();
    let logical_cores = read_u64(cpu, "cores").unwrap_or(0);
    let physical_cores = physical_core_count
        .map(|count| count as u64)
        .unwrap_or(logical_cores);
    let frequency_mhz = read_u64(cpu, "frequency_mhz").unwrap_or(0);

    json!({
        "name": brand,
        "brand": brand,
        "manufacturer": "Apple",
        "cores": physical_cores,
        "logical_cores": logical_cores,
        "threads": logical_cores,
        "frequency_mhz": frequency_mhz,
        "architecture": std::env::consts::ARCH,
        "socket": "",
        "processor_id": "",
    })
}

fn build_memory_info(memory: &Map<String, Value>) -> Value {
    json!({
        "total_bytes": read_u64(memory, "total_bytes").unwrap_or(0),
        "available_bytes": read_u64(memory, "available_bytes").unwrap_or(0),
        "slots_total": 0,
        "slots_used": 0,
        "speed_mhz": 0,
        "modules": [],
    })
}

fn build_hardware_disks(disks: &Value) -> Value {
    let Some(disks) = disks.as_array() else {
        return Value::Array(Vec::new());
    };
    Value::Array(
        disks
            .iter()
            .filter_map(|disk| {
                let obj = disk.as_object()?;
                let name = read_string(obj, "name").unwrap_or_default();
                let mount_point = read_string(obj, "mount_point").unwrap_or_default();
                let total_bytes = read_u64(obj, "total_bytes").unwrap_or(0);
                let available_bytes = read_u64(obj, "available_bytes").unwrap_or(0);
                let filesystem = read_string(obj, "file_system").unwrap_or_default();
                let used = total_bytes.saturating_sub(available_bytes);
                let percent_used = if total_bytes > 0 {
                    (used as f64 / total_bytes as f64) * 100.0
                } else {
                    0.0
                };

                Some(json!({
                    "device_id": name,
                    "model": name,
                    "serial_number": "",
                    "interface": "",
                    "media_type": "",
                    "size_bytes": total_bytes,
                    "smart": null,
                    "volumes": [{
                        "drive_letter": mount_point,
                        "label": mount_point,
                        "filesystem": filesystem,
                        "total_bytes": total_bytes,
                        "free_bytes": available_bytes,
                        "percent_used": percent_used,
                        "is_bitlocker_encrypted": null,
                    }],
                }))
            })
            .collect(),
    )
}

fn build_hardware_network_adapters(
    networks: &Value,
    hardware_ports: &HashMap<String, MacosNetworkHardwarePort>,
) -> Value {
    let Some(networks) = networks.as_array() else {
        return Value::Array(Vec::new());
    };
    Value::Array(
        networks
            .iter()
            .filter_map(|adapter| {
                let obj = adapter.as_object()?;
                let name = read_string(obj, "name")?;
                let hardware = hardware_ports.get(&name);
                let hardware_port = hardware
                    .map(|port| port.hardware_port.clone())
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| name.clone());
                let mac_address = hardware
                    .map(|port| port.ethernet_address.clone())
                    .filter(|value| !value.is_empty())
                    .unwrap_or_default();
                let ips = obj
                    .get("ips")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                let gateways = obj
                    .get("gateways")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                let status = if ips.is_empty() && gateways.is_empty() {
                    "down"
                } else {
                    "up"
                };

                Some(json!({
                    "name": hardware_port,
                    "description": hardware_port,
                    "interface_name": name,
                    "mac_address": mac_address,
                    "manufacturer": infer_network_adapter_manufacturer(&hardware_port),
                    "adapter_type": infer_network_adapter_type(&hardware_port),
                    "status": status,
                    "speed_mbps": null,
                    "is_physical": true,
                    "source": "networksetup",
                }))
            })
            .collect(),
    )
}

fn build_network_adapters(
    networks: &Value,
    hardware_ports: &HashMap<String, MacosNetworkHardwarePort>,
) -> Value {
    let Some(networks) = networks.as_array() else {
        return Value::Array(Vec::new());
    };
    Value::Array(
        networks
            .iter()
            .filter_map(|adapter| {
                let obj = adapter.as_object()?;
                let name = read_string(obj, "name")?;
                let mac_address = hardware_ports
                    .get(&name)
                    .map(|port| port.ethernet_address.clone())
                    .filter(|value| !value.is_empty())
                    .unwrap_or_default();
                let ips = obj
                    .get("ips")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                let gateways = obj
                    .get("gateways")
                    .cloned()
                    .unwrap_or_else(|| Value::Array(Vec::new()));
                let dns_servers = obj
                    .get("dns_servers")
                    .cloned()
                    .unwrap_or_else(|| Value::Array(Vec::new()));

                Some(json!({
                    "name": name,
                    "description": name,
                    "mac_address": mac_address,
                    "ips": ips.into_iter().filter_map(|ip| {
                        let ip_obj = ip.as_object()?;
                        Some(json!({
                            "address": read_string(ip_obj, "address").unwrap_or_default(),
                            "family": read_string(ip_obj, "family").unwrap_or_default(),
                            "prefix": read_u64(ip_obj, "prefix").unwrap_or(0),
                            "is_dhcp": false,
                            "dhcp_server": null,
                            "lease_obtained": null,
                            "lease_expires": null,
                        }))
                    }).collect::<Vec<_>>(),
                    "gateways": gateways,
                    "dns_servers": dns_servers,
                    "dns_suffix": "",
                    "status": "up",
                    "speed_mbps": null,
                    "mtu": null,
                }))
            })
            .collect(),
    )
}

pub(crate) async fn collect_network_adapters() -> Vec<Value> {
    let mut sys = System::new_all();
    let mut disks = Disks::new_with_refreshed_list();
    let mut networks = Networks::new_with_refreshed_list();
    let inventory = super::collect_inventory(&mut sys, &mut disks, &mut networks);
    let collection = serde_json::to_value(&inventory).unwrap_or_else(|_| json!({}));
    let networks_value = collection
        .get("networks")
        .cloned()
        .unwrap_or_else(|| Value::Array(Vec::new()));
    let hardware_ports = collect_network_hardware_ports().await;

    match build_network_adapters(&networks_value, &hardware_ports) {
        Value::Array(items) => items,
        _ => Vec::new(),
    }
}

fn infer_network_adapter_manufacturer(hardware_port: &str) -> &'static str {
    let normalized = hardware_port.to_ascii_lowercase();
    if normalized.contains("wi-fi")
        || normalized.contains("wifi")
        || normalized.contains("thunderbolt")
        || normalized.contains("ethernet")
    {
        "Apple"
    } else {
        ""
    }
}

fn infer_network_adapter_type(hardware_port: &str) -> &'static str {
    let normalized = hardware_port.to_ascii_lowercase();
    if normalized.contains("wi-fi") || normalized.contains("wifi") {
        "wireless"
    } else if normalized.contains("bluetooth") {
        "bluetooth"
    } else if normalized.contains("thunderbolt") {
        "thunderbolt"
    } else if normalized.contains("ethernet") {
        "ethernet"
    } else {
        "other"
    }
}

fn build_proxy_info(proxy: &MacosProxySettings) -> Value {
    json!({
        "enabled": proxy.enabled,
        "auto_detect": proxy.auto_detect,
        "proxy_server": proxy.proxy_server,
        "bypass_list": proxy.bypass_list,
        "pac_url": proxy.pac_url,
    })
}

fn build_services_info(services: &[Value]) -> Value {
    let running_count = services
        .iter()
        .filter(|service| {
            service
                .get("status")
                .and_then(Value::as_str)
                .map(|status| status == "running")
                .unwrap_or(false)
        })
        .count();
    let critical_services = services
        .iter()
        .filter(|service| {
            service
                .get("is_critical")
                .and_then(Value::as_bool)
                .unwrap_or(false)
        })
        .cloned()
        .collect::<Vec<_>>();

    json!({
        "total_count": services.len(),
        "running_count": running_count,
        "stopped_count": services.len().saturating_sub(running_count),
        "auto_start_count": services.len(),
        "services": services,
        "critical_services": critical_services,
    })
}

fn build_updates_info(
    pending_updates: &[Value],
    update_history: &[Value],
    reboot_required: bool,
    automatic_updates_enabled: bool,
) -> Value {
    let software_update = build_software_update_summary(
        pending_updates,
        update_history,
        reboot_required,
        automatic_updates_enabled,
    );
    json!({
        "software_update": software_update.clone(),
        "macos_software_update": software_update,
        "windows_update": {
            "pending_updates": pending_updates,
            "pending": pending_updates,
            "pending_count": pending_updates.len(),
            "pending_reboot": reboot_required,
            "automatic_updates_enabled": automatic_updates_enabled,
            "update_server": null,
            "service_status": "available",
            "last_scan": null,
            "last_successful_install": latest_history_date(update_history),
            "au_options": if automatic_updates_enabled { "AutomaticCheckEnabled" } else { "Manual" },
            "wu_server": null,
            "use_wu_server": false,
        },
        "update_history": update_history,
        "optional_updates": [],
        "driver_updates": [],
        "todo_data_collection": [],
    })
}

fn build_windows_update_summary(
    pending_updates: &[Value],
    update_history: &[Value],
    reboot_required: bool,
    automatic_updates_enabled: bool,
) -> Value {
    build_software_update_summary(
        pending_updates,
        update_history,
        reboot_required,
        automatic_updates_enabled,
    )
}

fn build_software_update_summary(
    pending_updates: &[Value],
    update_history: &[Value],
    reboot_required: bool,
    automatic_updates_enabled: bool,
) -> Value {
    json!({
        "pending_updates": pending_updates,
        "pending": pending_updates,
        "installed_count": update_history.len(),
        "last_install_date": latest_history_date(update_history),
        "pending_count": pending_updates.len(),
        "pending_reboot": reboot_required,
        "reboot_required": reboot_required,
        "automatic_updates_enabled": automatic_updates_enabled,
        "update_server": null,
        "source": "softwareupdate",
    })
}

pub(crate) fn macos_reboot_required(pending_updates: &[Value]) -> bool {
    pending_updates.iter().any(|update| {
        update
            .get("requires_reboot")
            .and_then(Value::as_bool)
            .unwrap_or(false)
    }) || [
        "/var/db/.AppleSoftwareUpdateRestartRequired",
        "/Library/Updates/.AppleSoftwareUpdateRestartRequired",
    ]
    .iter()
    .any(|path| Path::new(path).exists())
}

async fn run_command(program: &str, args: &[&str]) -> Option<String> {
    run_command_with_timeout(program, args, COMMAND_TIMEOUT).await
}

fn macos_command_path(program: &str) -> &str {
    let absolute = match program {
        "launchctl" => Some("/bin/launchctl"),
        "networksetup" => Some("/usr/sbin/networksetup"),
        "plutil" => Some("/usr/bin/plutil"),
        "scutil" => Some("/usr/sbin/scutil"),
        "softwareupdate" => Some("/usr/sbin/softwareupdate"),
        "sw_vers" => Some("/usr/bin/sw_vers"),
        "system_profiler" => Some("/usr/sbin/system_profiler"),
        _ => None,
    };
    absolute
        .filter(|path| Path::new(path).exists())
        .unwrap_or(program)
}

async fn run_command_with_timeout(
    program: &str,
    args: &[&str],
    duration: Duration,
) -> Option<String> {
    let command_path = macos_command_path(program);
    let output = timeout(
        duration,
        Command::new(command_path)
            .args(args)
            .kill_on_drop(true)
            .output(),
    )
    .await
    .ok()?
    .ok()?;

    if !output.status.success() && output.stdout.is_empty() && output.stderr.is_empty() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    if stderr.trim().is_empty() {
        Some(stdout.to_string())
    } else if stdout.trim().is_empty() {
        Some(stderr.to_string())
    } else {
        Some(format!("{stdout}\n{stderr}"))
    }
}

fn as_object_value(collection: &Map<String, Value>, key: &str) -> Map<String, Value> {
    collection
        .get(key)
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default()
}

fn read_string(map: &Map<String, Value>, key: &str) -> Option<String> {
    map.get(key).and_then(|value| text_value(Some(value)))
}

fn read_u64(map: &Map<String, Value>, key: &str) -> Option<u64> {
    map.get(key).and_then(|value| {
        value
            .as_u64()
            .or_else(|| value.as_i64().and_then(|v| u64::try_from(v).ok()))
            .or_else(|| value.as_str().and_then(|v| v.parse::<u64>().ok()))
    })
}

fn find_text_by_key(value: &Value, key: &str) -> Option<String> {
    match value {
        Value::Object(map) => {
            if let Some(found) = text_value(map.get(key)) {
                return Some(found);
            }
            map.values().find_map(|value| find_text_by_key(value, key))
        }
        Value::Array(items) => items.iter().find_map(|value| find_text_by_key(value, key)),
        _ => None,
    }
}

fn text_value(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::String(value) => {
            let trimmed = value.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            }
        }
        Value::Number(value) => Some(value.to_string()),
        Value::Bool(value) => Some(value.to_string()),
        _ => None,
    }
}

fn parse_bool_text(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "yes" | "true" | "spdisplays_yes" => Some(true),
        "0" | "no" | "false" | "spdisplays_no" => Some(false),
        _ => None,
    }
}

fn parse_u64_prefix(value: &str) -> Option<u64> {
    let digits = value
        .trim()
        .chars()
        .skip_while(|ch| !ch.is_ascii_digit())
        .take_while(|ch| ch.is_ascii_digit())
        .collect::<String>();
    if digits.is_empty() {
        None
    } else {
        digits.parse::<u64>().ok()
    }
}

fn first_string_in_array(value: Option<&Value>) -> Option<String> {
    value?
        .as_array()?
        .iter()
        .find_map(|item| item.as_str().map(str::trim).filter(|s| !s.is_empty()))
        .map(str::to_string)
}

fn normalize_text(value: &str) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase()
}

fn parse_size_text(value: &str) -> Option<u64> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    let split_at = value
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(value.len());
    let number = value[..split_at].trim().parse::<f64>().ok()?;
    let unit = value[split_at..]
        .trim()
        .trim_matches(|c: char| c == '(' || c == ')')
        .to_ascii_lowercase();
    let multiplier = if unit.starts_with("kib") || unit == "kb" || unit == "k" {
        1024.0
    } else if unit.starts_with("mib") || unit == "mb" || unit == "m" {
        1024.0 * 1024.0
    } else if unit.starts_with("gib") || unit == "gb" || unit == "g" {
        1024.0 * 1024.0 * 1024.0
    } else if unit.starts_with("tib") || unit == "tb" || unit == "t" {
        1024.0 * 1024.0 * 1024.0 * 1024.0
    } else {
        1.0
    };
    Some((number * multiplier).max(0.0).round() as u64)
}

fn epoch_to_rfc3339(seconds: u64) -> Option<String> {
    DateTime::<Utc>::from_timestamp(seconds as i64, 0).map(|dt| dt.to_rfc3339())
}

fn latest_history_date(update_history: &[Value]) -> Option<String> {
    update_history
        .iter()
        .filter_map(|entry| {
            entry
                .get("installed_at")
                .or_else(|| entry.get("installedAt"))
                .or_else(|| entry.get("date"))
                .and_then(Value::as_str)
        })
        .max()
        .map(str::to_string)
}

fn local_timezone_name() -> String {
    let Ok(path) = fs::read_link("/etc/localtime") else {
        return String::new();
    };
    let text = path.to_string_lossy();
    text.split("zoneinfo/")
        .nth(1)
        .map(str::to_string)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_sw_vers_output() {
        let parsed = parse_sw_vers_output(
            "ProductName:\t\tmacOS\nProductVersion:\t\t14.5\nBuildVersion:\t\t23F79\n",
        );

        assert_eq!(parsed.product_name, "macOS");
        assert_eq!(parsed.product_version, "14.5");
        assert_eq!(parsed.build_version, "23F79");
    }

    #[test]
    fn parses_system_profiler_hardware_identity() {
        let hardware = parse_system_profiler_hardware(
            r#"{
              "SPHardwareDataType": [
                {
                  "machine_name": "MacBook Pro",
                  "machine_model": "Mac15,3",
                  "chip_type": "Apple M3 Pro",
                  "physical_memory": "18 GB",
                  "serial_number": "C02TEST12345",
                  "platform_UUID": "11111111-2222-3333-4444-555555555555",
                  "provisioning_UDID": "00008122-001A2B3C4D58001E",
                  "activation_lock_status": "activation_lock_disabled"
                }
              ]
            }"#,
        );

        assert_eq!(hardware.model_name.as_deref(), Some("MacBook Pro"));
        assert_eq!(hardware.model_identifier.as_deref(), Some("Mac15,3"));
        assert_eq!(hardware.chip.as_deref(), Some("Apple M3 Pro"));
        assert_eq!(hardware.serial_number.as_deref(), Some("C02TEST12345"));
        assert_eq!(
            hardware.hardware_uuid.as_deref(),
            Some("11111111-2222-3333-4444-555555555555")
        );
    }

    #[test]
    fn parses_system_profiler_power_battery() {
        let battery = parse_system_profiler_power(
            r#"{
              "SPPowerDataType": [
                {
                  "sppower_battery_charge_info": {
                    "sppower_battery_state_of_charge": "87%",
                    "sppower_battery_is_charging": "No",
                    "sppower_battery_fully_charged": "No",
                    "sppower_battery_time_remaining": "2:31"
                  },
                  "sppower_battery_health_info": {
                    "sppower_battery_cycle_count": "42",
                    "sppower_battery_condition": "Normal",
                    "sppower_battery_health": "Good"
                  },
                  "sppower_ac_charger_connected": "Yes"
                }
              ]
            }"#,
        )
        .expect("battery info");

        assert!(battery.present);
        assert_eq!(battery.percentage, Some(87));
        assert_eq!(battery.cycle_count, Some(42));
        assert_eq!(battery.is_charging, Some(false));
        assert_eq!(battery.ac_power_connected, Some(true));
        assert_eq!(battery.condition.as_deref(), Some("Normal"));
    }

    #[test]
    fn parses_system_profiler_applications() {
        let apps = parse_system_profiler_applications(
            r#"{
              "SPApplicationsDataType": [
                {
                  "_name": "Safari",
                  "version": "17.5",
                  "obtained_from": "Apple",
                  "path": "/Applications/Safari.app",
                  "size": "19.4 MB",
                  "arch_kind": "Universal"
                }
              ]
            }"#,
        );

        assert_eq!(apps.len(), 1);
        assert_eq!(apps[0]["name"], "Safari");
        assert_eq!(apps[0]["version"], "17.5");
        assert_eq!(apps[0]["size_bytes"], 20_342_374);
    }

    #[test]
    fn parses_system_profiler_display_gpus() {
        let gpus = parse_system_profiler_displays(
            r#"{
              "SPDisplaysDataType": [
                {
                  "_name": "Apple M3",
                  "sppci_model": "Apple M3",
                  "spdisplays_vendor": "Apple",
                  "spdisplays_vram": "18 GB",
                  "spdisplays_metal": "Metal 3",
                  "spdisplays_device-id": "0x1234",
                  "spdisplays_ndrvs": [
                    {
                      "_name": "Built-in Liquid Retina Display",
                      "_spdisplays_resolution": "3024 x 1964 Retina",
                      "spdisplays_main": "spdisplays_yes",
                      "spdisplays_built-in": "spdisplays_yes"
                    }
                  ]
                }
              ]
            }"#,
        );

        assert_eq!(gpus.len(), 1);
        assert_eq!(gpus[0]["name"], "Apple M3");
        assert_eq!(gpus[0]["vendor"], "Apple");
        assert_eq!(gpus[0]["vram_bytes"], 19_327_352_832_u64);
        assert_eq!(gpus[0]["metal"], "Metal 3");
        assert_eq!(
            gpus[0]["displays"][0]["name"],
            "Built-in Liquid Retina Display"
        );
        assert_eq!(gpus[0]["displays"][0]["resolution"], "3024 x 1964 Retina");
        assert_eq!(gpus[0]["displays"][0]["main"], true);
        assert_eq!(gpus[0]["displays"][0]["built_in"], true);
    }

    #[test]
    fn parses_networksetup_hardware_ports() {
        let ports = parse_network_hardware_ports(
            "Hardware Port: Wi-Fi\nDevice: en0\nEthernet Address: aa:bb:cc:dd:ee:ff\n\nHardware Port: Thunderbolt Bridge\nDevice: bridge0\nEthernet Address: 11:22:33:44:55:66\n",
        );

        assert_eq!(ports.len(), 2);
        assert_eq!(ports["en0"].hardware_port, "Wi-Fi");
        assert_eq!(ports["en0"].ethernet_address, "aa:bb:cc:dd:ee:ff");
        assert_eq!(ports["bridge0"].hardware_port, "Thunderbolt Bridge");
    }

    #[test]
    fn builds_macos_network_hardware_adapters() {
        let networks = json!([
            {
                "name": "en0",
                "ips": [{ "address": "192.168.1.20", "family": "ipv4", "prefix": 24 }],
                "gateways": ["192.168.1.1"],
                "dns_servers": ["1.1.1.1"]
            }
        ]);
        let hardware_ports = parse_network_hardware_ports(
            "Hardware Port: Wi-Fi\nDevice: en0\nEthernet Address: aa:bb:cc:dd:ee:ff\n",
        );

        let hardware_adapters = build_hardware_network_adapters(&networks, &hardware_ports);
        let network_adapters = build_network_adapters(&networks, &hardware_ports);

        assert_eq!(hardware_adapters[0]["name"], "Wi-Fi");
        assert_eq!(hardware_adapters[0]["interface_name"], "en0");
        assert_eq!(hardware_adapters[0]["adapter_type"], "wireless");
        assert_eq!(hardware_adapters[0]["mac_address"], "aa:bb:cc:dd:ee:ff");
        assert_eq!(network_adapters[0]["mac_address"], "aa:bb:cc:dd:ee:ff");
    }

    #[test]
    fn parses_scutil_proxy_settings() {
        let proxy = parse_scutil_proxy_settings(
            r#"<dictionary> {
  ExceptionsList : <array> {
    0 : *.local
    1 : 169.254/16
  }
  HTTPEnable : 1
  HTTPPort : 8080
  HTTPProxy : proxy.example.test
  HTTPSEnable : 1
  HTTPSPort : 8443
  HTTPSProxy : secure-proxy.example.test
  ProxyAutoConfigEnable : 1
  ProxyAutoConfigURLString : https://proxy.example.test/proxy.pac
  ProxyAutoDiscoveryEnable : 1
}"#,
        );

        assert!(proxy.enabled);
        assert!(proxy.auto_detect);
        assert_eq!(
            proxy.proxy_server.as_deref(),
            Some("https://secure-proxy.example.test:8443")
        );
        assert_eq!(
            proxy.pac_url.as_deref(),
            Some("https://proxy.example.test/proxy.pac")
        );
        assert_eq!(proxy.bypass_list, vec!["*.local", "169.254/16"]);
    }

    #[test]
    fn parses_launchctl_list_output() {
        let services = parse_launchctl_list_output(
            "PID\tStatus\tLabel\n123\t0\tcom.example.running\n-\t78\tcom.example.exited\n-\t0\tcom.example.stopped\n",
        );

        assert_eq!(services.len(), 3);
        assert_eq!(services[0]["status"], "running");
        assert_eq!(services[1]["status"], "exited(78)");
        assert_eq!(services[2]["status"], "stopped");
    }

    #[test]
    fn parses_launchd_plist_json() {
        let value = json!({
            "Label": "com.example.agent",
            "ProgramArguments": ["/usr/local/bin/agent", "--flag value"],
            "Disabled": false
        });
        let item = startup_item_from_plist_json(
            &value,
            Path::new("/Library/LaunchDaemons/a.plist"),
            "root",
        )
        .expect("startup item");

        assert_eq!(item["label"], "com.example.agent");
        assert_eq!(item["command"], "/usr/local/bin/agent \"--flag value\"");
        assert_eq!(item["is_enabled"], true);
    }

    #[test]
    fn parses_softwareupdate_list_output() {
        let updates = parse_softwareupdate_list_output(
            "Software Update Tool\n\n* Label: macOS Sonoma 14.5-23F79\n    Title: macOS Sonoma 14.5, Version: 14.5, Size: 3846061KiB, Recommended: YES, Action: restart,\n",
        );

        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0]["title"], "macOS Sonoma 14.5");
        assert_eq!(updates[0]["version"], "14.5");
        assert_eq!(updates[0]["requires_reboot"], true);
        assert_eq!(updates[0]["requiresReboot"], true);
        assert_eq!(updates[0]["updateKey"], "macos sonoma 14.5|");
        assert_eq!(updates[0]["label"], "macOS Sonoma 14.5-23F79");
        assert_eq!(updates[0]["size_bytes"], 3_938_366_464_u64);
    }

    #[test]
    fn parses_softwareupdate_titles_with_commas() {
        let updates = parse_softwareupdate_list_output(
            "Software Update Tool\n\n* Label: Example App Update-1.2\n    Title: Example App, Security Update, Version: 1.2, Size: 12345KiB, Recommended: YES, Action: none,\n",
        );

        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0]["title"], "Example App, Security Update");
        assert_eq!(updates[0]["version"], "1.2");
        assert_eq!(updates[0]["updateKey"], "example app, security update|");
    }

    #[test]
    fn parses_softwareupdate_list_output_case_insensitively() {
        let updates = parse_softwareupdate_list_output(
            "software update tool\n\n* label: Safari17.5.1-17618.2.12.111.5\n    title: Safari, version: 17.5.1, size: 120000KiB, recommended: YES, action: restart,\n",
        );

        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0]["label"], "Safari17.5.1-17618.2.12.111.5");
        assert_eq!(updates[0]["title"], "Safari");
        assert_eq!(updates[0]["version"], "17.5.1");
        assert_eq!(updates[0]["requires_reboot"], true);
        assert_eq!(updates[0]["recommended"], true);
        assert_eq!(updates[0]["size_bytes"], 122_880_000_u64);
    }

    #[test]
    fn filters_macos_minor_update_when_newer_major_is_available() {
        let updates = parse_softwareupdate_list_output(
            "Software Update Tool\n\n* Label: macOS Tahoe 26.5.1-25F5057\n    Title: macOS Tahoe 26.5.1, Version: 26.5.1, Size: 8000000KiB, Recommended: YES, Action: restart,\n* Label: macOS Ventura 13.7.8-22H730\n    Title: macOS Ventura 13.7.8, Version: 13.7.8, Size: 4000000KiB, Recommended: YES, Action: restart,\n* Label: Safari18.5-18621\n    Title: Safari, Version: 18.5, Size: 120000KiB, Recommended: YES, Action: none,\n* Label: macOS Security Response 14.5-a\n    Title: macOS Security Response 14.5, Version: 14.5, Size: 1000KiB, Recommended: YES, Action: restart,\n",
        );

        let titles = updates
            .iter()
            .filter_map(|update| update.get("title").and_then(Value::as_str))
            .collect::<Vec<_>>();
        assert!(titles.contains(&"macOS Tahoe 26.5.1"));
        assert!(titles.contains(&"Safari"));
        assert!(titles.contains(&"macOS Security Response 14.5"));
        assert!(!titles.contains(&"macOS Ventura 13.7.8"));
    }

    #[test]
    fn parses_system_profiler_install_history() {
        let history = parse_system_profiler_install_history(
            r#"{
  "SPInstallHistoryDataType": [
    {
      "_name": "macOS Sonoma 14.5",
      "install_date": "2024-05-13 12:34:56 +0000",
      "install_version": "14.5",
      "package_source": "Apple",
      "content_download_type": "software_update"
    },
    {
      "_name": "Command Line Tools for Xcode",
      "install_date": "2024-04-01T08:00:00Z",
      "install_version": "15.3",
      "package_source": "Apple"
    }
  ]
}"#,
        );

        assert_eq!(history.len(), 2);
        assert_eq!(history[0]["title"], "macOS Sonoma 14.5");
        assert_eq!(history[0]["version"], "14.5");
        assert_eq!(history[0]["installed_at"], "2024-05-13T12:34:56+00:00");
        assert_eq!(history[0]["result"], "succeeded");
        assert_eq!(history[0]["updateKey"], "macos sonoma 14.5|");
        assert_eq!(history[1]["installed_at"], "2024-04-01T08:00:00+00:00");
    }

    #[test]
    fn builds_backend_readable_collection_shape() {
        let inventory = json!({
            "system": {
                "hostname": "mac-1",
                "os_name": "Darwin",
                "os_version": "14.5",
                "kernel_version": "23F79",
                "architecture": "aarch64",
                "uptime_seconds": 100,
                "boot_time": 1_700_000_000_u64
            },
            "cpu": {
                "brand": "Apple M3",
                "cores": 8,
                "frequency_mhz": 0
            },
            "memory": {
                "total_bytes": 17179869184_u64,
                "available_bytes": 8589934592_u64
            },
            "disks": [{
                "name": "disk3s1",
                "mount_point": "/",
                "total_bytes": 1000,
                "available_bytes": 250,
                "file_system": "apfs"
            }],
            "networks": [{
                "name": "en0",
                "ips": [{ "address": "192.168.1.20", "family": "ipv4", "prefix": 24 }],
                "gateways": ["192.168.1.1"],
                "dns_servers": ["1.1.1.1"]
            }]
        });

        let collection = build_normalized_collection(
            inventory,
            Some(4),
            &SwVers {
                product_name: "macOS".to_string(),
                product_version: "14.5".to_string(),
                build_version: "23F79".to_string(),
            },
            &MacosHardwareOverview {
                model_name: Some("MacBook Pro".to_string()),
                model_identifier: Some("Mac15,3".to_string()),
                chip: Some("Apple M3 Pro".to_string()),
                memory: Some("18 GB".to_string()),
                serial_number: Some("C02TEST12345".to_string()),
                hardware_uuid: Some("11111111-2222-3333-4444-555555555555".to_string()),
                ..Default::default()
            },
            vec![json!({ "name": "Safari" })],
            vec![json!({ "name": "Apple M3", "vendor": "Apple" })],
            parse_network_hardware_ports(
                "Hardware Port: Wi-Fi\nDevice: en0\nEthernet Address: aa:bb:cc:dd:ee:ff\n",
            ),
            MacosProxySettings {
                enabled: true,
                auto_detect: true,
                proxy_server: Some("http://proxy.example.test:8080".to_string()),
                bypass_list: vec!["*.local".to_string()],
                pac_url: Some("https://proxy.example.test/proxy.pac".to_string()),
            },
            Some(MacosBatteryInfo {
                present: true,
                percentage: Some(87),
                is_charging: Some(false),
                fully_charged: Some(false),
                cycle_count: Some(42),
                condition: Some("Normal".to_string()),
                health: Some("Good".to_string()),
                ac_power_connected: Some(true),
                time_remaining: Some("2:31".to_string()),
            }),
            vec![json!({ "name": "com.example.service", "status": "running" })],
            vec![json!({ "name": "com.example.service", "command": "/bin/true", "location": "/Library/LaunchDaemons/a.plist" })],
            vec![json!({ "title": "macOS 14.5", "requires_reboot": true, "source": "softwareupdate" })],
            vec![json!({ "title": "macOS 14.4.1", "installed_at": "2024-04-01T10:00:00Z", "source": "system_profiler" })],
            true,
        )
        .expect("normalized collection");

        assert_eq!(
            collection["operating_system"]["system"]["os"]["name"],
            "macOS"
        );
        assert_eq!(
            collection["operating_system"]["system"]["os"]["serial_number"],
            "C02TEST12345"
        );
        assert_eq!(collection["hardware"]["motherboard"]["product"], "Mac15,3");
        assert_eq!(
            collection["hardware"]["motherboard"]["uuid"],
            "11111111-2222-3333-4444-555555555555"
        );
        assert_eq!(collection["hardware"]["cpu"]["cores"], 4);
        assert_eq!(
            collection["software"]["installed_programs"][0]["name"],
            "Safari"
        );
        assert_eq!(collection["hardware"]["gpus"][0]["name"], "Apple M3");
        assert_eq!(
            collection["hardware"]["network_adapters"][0]["name"],
            "Wi-Fi"
        );
        assert_eq!(collection["hardware"]["battery"]["percentage"], 87);
        assert_eq!(collection["hardware"]["battery"]["cycle_count"], 42);
        assert_eq!(collection["hardware"]["battery"]["condition"], "Normal");
        assert_eq!(
            collection["network"]["adapters"][0]["mac_address"],
            "aa:bb:cc:dd:ee:ff"
        );
        assert_eq!(collection["network"]["proxy"]["enabled"], true);
        assert_eq!(
            collection["network"]["proxy"]["proxy_server"],
            "http://proxy.example.test:8080"
        );
        assert_eq!(collection["network"]["proxy"]["bypass_list"][0], "*.local");
        assert_eq!(
            collection["operating_system"]["updates"]["windows_update"]["pending_count"],
            1
        );
        assert_eq!(
            collection["operating_system"]["updates"]["software_update"]["pending_count"],
            1
        );
        assert_eq!(
            collection["operating_system"]["updates"]["macos_software_update"]["source"],
            "softwareupdate"
        );
        assert_eq!(
            collection["software"]["software_updates"]["pending_updates"][0]["source"],
            "softwareupdate"
        );
        assert_eq!(collection["software"]["macos_updates"]["pending_count"], 1);
        assert_eq!(
            collection["operating_system"]["updates"]["update_history"][0]["title"],
            "macOS 14.4.1"
        );
        assert_eq!(
            collection["software"]["macos_updates"]["installed_count"],
            1
        );
        assert!(collection["unsupported_features"]["remote_desktop"].is_null());
        assert_eq!(
            collection["unsupported_features"]["remote_registry"],
            "unsupported_platform"
        );
    }

    #[test]
    fn macos_command_path_prefers_absolute_service_paths() {
        if Path::new("/usr/sbin/softwareupdate").exists() {
            assert_eq!(
                macos_command_path("softwareupdate"),
                "/usr/sbin/softwareupdate"
            );
        }
        if Path::new("/usr/sbin/system_profiler").exists() {
            assert_eq!(
                macos_command_path("system_profiler"),
                "/usr/sbin/system_profiler"
            );
        }
        if Path::new("/bin/launchctl").exists() {
            assert_eq!(macos_command_path("launchctl"), "/bin/launchctl");
        }
    }

    #[test]
    fn macos_command_path_leaves_unknown_programs_unchanged() {
        assert_eq!(
            macos_command_path("definitely-not-a-talos-tool"),
            "definitely-not-a-talos-tool"
        );
    }
}
