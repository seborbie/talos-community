#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

use std::{
    collections::{HashMap, HashSet},
    fs,
    path::Path,
    time::{Duration, SystemTime},
};

use anyhow::{Context, Result};
use chrono::{DateTime, NaiveDateTime, Utc};
use serde_json::{json, Map, Value};
use sysinfo::{Disks, Networks, System};
use tokio::{process::Command, time::timeout};
use tracing::debug;

const COMMAND_TIMEOUT: Duration = Duration::from_secs(15);
const APT_METADATA_REFRESH_TIMEOUT: Duration = Duration::from_secs(120);
const APT_METADATA_CACHE_MAX_AGE: Duration = Duration::from_secs(60 * 60);
const APT_LISTS_DIR: &str = "/var/lib/apt/lists";
const MAX_APT_HISTORY_ITEMS: usize = 100;
const MAX_PACKAGES: usize = 5_000;

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
    let mut collection = serde_json::to_value(&inventory).context("serialize linux inventory")?;
    let collection_obj = collection
        .as_object_mut()
        .context("linux inventory did not serialize to an object")?;

    let installed_programs = collect_installed_programs().await;
    let (services, startup_items) = collect_services_and_startup_items().await;
    let pending_updates = collect_pending_updates().await;
    let update_history = collect_update_history().await;
    let reboot_required = linux_reboot_required();
    let automatic_updates_enabled = apt_automatic_updates_enabled();

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

    collection_obj.insert(
        "operating_system".to_string(),
        json!({
            "system": build_system_info(&system),
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
            "cpu": build_cpu_info(&cpu, sys.physical_core_count()),
            "memory": build_memory_info(&memory),
            "disks": build_hardware_disks(&disks_value),
            "gpus": [],
            "network_adapters": [],
            "tpm": null,
            "secure_boot": null,
            "battery": null,
            "motherboard": null,
            "todo_data_collection": [],
        }),
    );
    collection_obj.insert(
        "network".to_string(),
        json!({
            "adapters": build_network_adapters(&networks_value),
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
            "proxy": {
                "enabled": false,
                "auto_detect": false,
                "proxy_server": null,
                "bypass_list": [],
                "pac_url": null,
            },
            "firewall_rules_count": null,
            "todo_data_collection": [],
        }),
    );
    collection_obj.insert(
        "software".to_string(),
        json!({
            "installed_programs": installed_programs,
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
            "remote_desktop": "unsupported_platform",
            "remote_registry": "unsupported_platform",
            "chat": "unsupported_platform",
        }),
    );

    let snapshot = json!({
        "metadata": {
            "agent_id": agent_id,
            "device_name": hostname,
            "boot_session_id": boot_session_id,
            "agent_version": agent_version,
            "collection_profile": "linux_full",
            "timestamp": collected_at,
        },
        "collection": collection,
    });

    Ok((collected_at, snapshot))
}

fn as_object_value(collection: &Map<String, Value>, key: &str) -> Map<String, Value> {
    collection
        .get(key)
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default()
}

fn build_system_info(system: &Map<String, Value>) -> Value {
    let hostname = read_string(system, "hostname").unwrap_or_else(|| "unknown".to_string());
    let distro = read_string(system, "distro")
        .or_else(|| read_string(system, "os_version"))
        .unwrap_or_else(|| "Linux".to_string());
    let os_version = read_string(system, "os_version").unwrap_or_default();
    let kernel_version = read_string(system, "kernel_version").unwrap_or_default();
    let architecture =
        read_string(system, "architecture").unwrap_or_else(|| std::env::consts::ARCH.to_string());
    let uptime_seconds = read_u64(system, "uptime_seconds").unwrap_or(0);
    let boot_time = read_u64(system, "boot_time");

    json!({
        "hostname": hostname,
        "domain": null,
        "name": distro,
        "version": os_version,
        "os": {
            "name": distro,
            "version": os_version,
            "build": kernel_version,
            "edition": "",
            "install_date": null,
            "architecture": architecture,
            "locale": "",
            "timezone": "",
            "serial_number": null,
        },
        "boot_time": boot_time.and_then(epoch_to_rfc3339),
        "uptime_seconds": uptime_seconds,
        "todo_data_collection": [],
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
        "manufacturer": "",
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

fn build_network_adapters(networks: &Value) -> Value {
    let Some(networks) = networks.as_array() else {
        return Value::Array(Vec::new());
    };
    Value::Array(
        networks
            .iter()
            .filter_map(|adapter| {
                let obj = adapter.as_object()?;
                let name = read_string(obj, "name")?;
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
                    "mac_address": "",
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

fn build_services_info(services: &[Value]) -> Value {
    let running_count = services
        .iter()
        .filter(|service| {
            service
                .get("status")
                .and_then(Value::as_str)
                .map(|status| matches!(status, "running" | "active"))
                .unwrap_or(false)
        })
        .count();
    let auto_start_count = services
        .iter()
        .filter(|service| {
            service
                .get("start_type")
                .and_then(Value::as_str)
                .map(is_systemd_enabled_state)
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
        "auto_start_count": auto_start_count,
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
    json!({
        "windows_update": {
            "pending_updates": pending_updates,
            "pending": pending_updates,
            "pending_count": pending_updates.len(),
            "pending_reboot": reboot_required,
            "automatic_updates_enabled": automatic_updates_enabled,
            "update_server": null,
            "service_status": if package_manager_available() { "available" } else { "unavailable" },
            "last_scan": null,
            "last_successful_install": latest_history_date(update_history),
            "au_options": if automatic_updates_enabled { "UnattendedUpgrade" } else { "Manual" },
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
    json!({
        "installed_count": update_history.len(),
        "last_install_date": latest_history_date(update_history),
        "pending_count": pending_updates.len(),
        "pending_reboot": reboot_required,
        "automatic_updates_enabled": automatic_updates_enabled,
        "update_server": null,
    })
}

async fn collect_installed_programs() -> Vec<Value> {
    if let Some(stdout) = run_command(
        "dpkg-query",
        &[
            "-W",
            "-f=${binary:Package}\t${Version}\t${Maintainer}\t${Installed-Size}\t${db:Status-Abbrev}\n",
        ],
    )
    .await
    {
        let programs = parse_dpkg_query_output(&stdout);
        if !programs.is_empty() {
            return programs;
        }
    }

    if let Some(stdout) = run_command(
        "rpm",
        &[
            "-qa",
            "--qf",
            "%{NAME}\t%{VERSION}-%{RELEASE}\t%{VENDOR}\t%{SIZE}\t%{ARCH}\t%{INSTALLTIME}\n",
        ],
    )
    .await
    {
        let programs = parse_rpm_query_output(&stdout);
        if !programs.is_empty() {
            return programs;
        }
    }

    Vec::new()
}

async fn collect_services_and_startup_items() -> (Vec<Value>, Vec<Value>) {
    let service_unit_files = collect_unit_file_states("service").await;
    let timer_unit_files = collect_unit_file_states("timer").await;
    let units_stdout = run_command(
        "systemctl",
        &[
            "list-units",
            "--type=service",
            "--all",
            "--no-legend",
            "--no-pager",
        ],
    )
    .await
    .unwrap_or_default();
    let services = parse_systemd_services(&units_stdout, &service_unit_files);
    let startup_items = startup_items_from_unit_files(&service_unit_files, &timer_unit_files);
    (services, startup_items)
}

async fn collect_unit_file_states(unit_type: &str) -> HashMap<String, String> {
    let Some(stdout) = run_command(
        "systemctl",
        &[
            "list-unit-files",
            &format!("--type={unit_type}"),
            "--no-legend",
            "--no-pager",
        ],
    )
    .await
    else {
        return HashMap::new();
    };
    parse_systemd_unit_files(&stdout)
}

async fn collect_pending_updates() -> Vec<Value> {
    let reboot_required = linux_reboot_required();
    if apt_available() {
        refresh_apt_metadata_cache_if_needed().await;
    }
    if let Some(stdout) = run_command("apt", &["list", "--upgradable"]).await {
        return parse_apt_upgradable_output(&stdout, reboot_required);
    }
    if let Some(stdout) = run_command("dnf", &["check-update"]).await {
        let updates = parse_dnf_yum_check_update_output(&stdout, reboot_required);
        if !updates.is_empty() {
            return updates;
        }
    }
    if let Some(stdout) = run_command("yum", &["check-update"]).await {
        let updates = parse_dnf_yum_check_update_output(&stdout, reboot_required);
        if !updates.is_empty() {
            return updates;
        }
    }
    if let Some(stdout) = run_command("checkupdates", &[]).await {
        let updates = parse_pacman_updates_output(&stdout, reboot_required);
        if !updates.is_empty() {
            return updates;
        }
    }
    if let Some(stdout) = run_command("pacman", &["-Qu"]).await {
        return parse_pacman_updates_output(&stdout, reboot_required);
    }
    Vec::new()
}

async fn collect_update_history() -> Vec<Value> {
    let mut history = Vec::new();
    for path in ["/var/log/apt/history.log", "/var/log/apt/history.log.1"] {
        let Ok(contents) = fs::read_to_string(path) else {
            continue;
        };
        history.extend(parse_apt_history_output(
            &contents,
            MAX_APT_HISTORY_ITEMS.saturating_sub(history.len()),
        ));
        if history.len() >= MAX_APT_HISTORY_ITEMS {
            break;
        }
    }
    history.truncate(MAX_APT_HISTORY_ITEMS);
    history
}

async fn run_command(program: &str, args: &[&str]) -> Option<String> {
    run_command_with_timeout(program, args, COMMAND_TIMEOUT).await
}

async fn run_command_with_timeout(
    program: &str,
    args: &[&str],
    duration: Duration,
) -> Option<String> {
    let mut command = Command::new(program);
    if matches!(program, "apt" | "apt-get") {
        apply_apt_environment(&mut command);
    }
    let output = timeout(duration, command.args(args).kill_on_drop(true).output())
        .await
        .ok()?
        .ok()?;
    if !output.status.success() && output.stdout.is_empty() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).to_string())
}

fn apply_apt_environment(command: &mut Command) -> &mut Command {
    command
        .env("DEBIAN_FRONTEND", "noninteractive")
        .env("APT_LISTCHANGES_FRONTEND", "none")
}

async fn refresh_apt_metadata_cache_if_needed() {
    if !apt_get_available() || !running_as_root() || !apt_metadata_refresh_required() {
        return;
    }

    let args = [
        "-o",
        "DPkg::Lock::Timeout=60",
        "-o",
        "Acquire::Retries=2",
        "update",
    ];
    if run_command_with_timeout("apt-get", &args, APT_METADATA_REFRESH_TIMEOUT)
        .await
        .is_none()
    {
        debug!("apt metadata refresh failed; using existing apt package lists");
    }
}

fn apt_metadata_refresh_required() -> bool {
    let newest_list_mtime = newest_apt_list_mtime(Path::new(APT_LISTS_DIR));
    apt_metadata_refresh_required_from_mtime(newest_list_mtime, SystemTime::now())
}

fn apt_metadata_refresh_required_from_mtime(
    newest_list_mtime: Option<SystemTime>,
    now: SystemTime,
) -> bool {
    let Some(mtime) = newest_list_mtime else {
        return true;
    };
    now.duration_since(mtime)
        .map(|age| age >= APT_METADATA_CACHE_MAX_AGE)
        .unwrap_or(false)
}

fn newest_apt_list_mtime(path: &Path) -> Option<SystemTime> {
    fs::read_dir(path)
        .ok()?
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| {
            let file_type = entry.file_type().ok()?;
            if !file_type.is_file() {
                return None;
            }
            let name = entry.file_name();
            if name.to_string_lossy() == "lock" {
                return None;
            }
            entry.metadata().ok()?.modified().ok()
        })
        .max()
}

fn parse_dpkg_query_output(stdout: &str) -> Vec<Value> {
    let mut programs = Vec::new();
    let mut seen = HashSet::new();
    for line in stdout.lines() {
        let mut parts = line.split('\t');
        let Some(name) = parts
            .next()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            continue;
        };
        let version = parts.next().map(str::trim).unwrap_or_default();
        let publisher = parts.next().map(str::trim).unwrap_or_default();
        let size_kb = parts
            .next()
            .map(str::trim)
            .and_then(|value| value.parse::<u64>().ok());
        let status = parts.next().map(str::trim).unwrap_or_default();
        if !status.starts_with("ii") || !seen.insert(name.to_string()) {
            continue;
        }
        programs.push(json!({
            "name": name,
            "publisher": publisher,
            "version": version,
            "install_date": null,
            "size_bytes": size_kb.map(|size| size.saturating_mul(1024)),
            "source": "dpkg",
            "location": null,
            "uninstall_string": format!("apt remove {name}"),
            "is_64_bit": package_is_64_bit(name),
        }));
        if programs.len() >= MAX_PACKAGES {
            break;
        }
    }
    programs.sort_by(|a, b| {
        a.get("name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .cmp(b.get("name").and_then(Value::as_str).unwrap_or_default())
    });
    programs
}

fn parse_rpm_query_output(stdout: &str) -> Vec<Value> {
    let mut programs = Vec::new();
    let mut seen = HashSet::new();
    for line in stdout.lines() {
        let mut parts = line.split('\t');
        let Some(name) = parts
            .next()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            continue;
        };
        let version = parts.next().map(str::trim).unwrap_or_default();
        let vendor = parts.next().map(str::trim).unwrap_or_default();
        let size_bytes = parts
            .next()
            .map(str::trim)
            .and_then(|value| value.parse::<u64>().ok());
        let arch = parts.next().map(str::trim).unwrap_or_default();
        let install_time = parts
            .next()
            .map(str::trim)
            .and_then(|value| value.parse::<u64>().ok())
            .and_then(epoch_to_rfc3339);
        let identity = if arch.is_empty() {
            name.to_string()
        } else {
            format!("{name}.{arch}")
        };
        if !seen.insert(identity) {
            continue;
        }

        let publisher = if vendor.is_empty() || vendor == "(none)" {
            Value::Null
        } else {
            json!(vendor)
        };
        let architecture = if arch.is_empty() {
            Value::Null
        } else {
            json!(arch)
        };

        programs.push(json!({
            "name": name,
            "publisher": publisher,
            "version": version,
            "install_date": install_time,
            "size_bytes": size_bytes,
            "source": "rpm",
            "location": null,
            "uninstall_string": format!("dnf remove {name}"),
            "architecture": architecture,
            "is_64_bit": package_arch_is_64_bit(arch),
        }));
        if programs.len() >= MAX_PACKAGES {
            break;
        }
    }
    programs.sort_by(|a, b| {
        a.get("name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .cmp(b.get("name").and_then(Value::as_str).unwrap_or_default())
    });
    programs
}

fn parse_systemd_unit_files(stdout: &str) -> HashMap<String, String> {
    let mut states = HashMap::new();
    for line in stdout.lines() {
        let columns = line.split_whitespace().collect::<Vec<_>>();
        if columns.len() < 2 {
            continue;
        }
        states.insert(columns[0].to_string(), columns[1].to_string());
    }
    states
}

fn parse_systemd_services(stdout: &str, unit_file_states: &HashMap<String, String>) -> Vec<Value> {
    let mut by_name = HashMap::<String, Value>::new();
    for line in stdout.lines() {
        let columns = line.split_whitespace().collect::<Vec<_>>();
        if columns.len() < 4 {
            continue;
        }
        let unit = columns[0];
        if !unit.ends_with(".service") {
            continue;
        }
        let active = columns[2];
        let sub = columns[3];
        let description = if columns.len() > 4 {
            columns[4..].join(" ")
        } else {
            unit.trim_end_matches(".service").to_string()
        };
        by_name.insert(
            unit.to_string(),
            service_json(
                unit,
                &description,
                service_status(active, sub),
                unit_file_states
                    .get(unit)
                    .map(String::as_str)
                    .unwrap_or("unknown"),
            ),
        );
    }

    for (unit, state) in unit_file_states {
        if unit.ends_with(".service") && !by_name.contains_key(unit) {
            by_name.insert(
                unit.clone(),
                service_json(
                    unit,
                    unit.trim_end_matches(".service"),
                    "inactive",
                    state.as_str(),
                ),
            );
        }
    }

    let mut services = by_name.into_values().collect::<Vec<_>>();
    services.sort_by(|a, b| {
        a.get("name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .cmp(b.get("name").and_then(Value::as_str).unwrap_or_default())
    });
    services
}

fn service_json(unit: &str, description: &str, status: &str, start_type: &str) -> Value {
    json!({
        "name": unit,
        "display_name": description,
        "status": status,
        "start_type": start_type,
        "account": "root",
        "process_id": null,
        "can_stop": null,
        "can_pause": null,
        "description": description,
        "path": null,
        "is_critical": is_critical_linux_service(unit),
    })
}

fn startup_items_from_unit_files(
    service_unit_files: &HashMap<String, String>,
    timer_unit_files: &HashMap<String, String>,
) -> Vec<Value> {
    let mut items = Vec::new();
    for (unit, state) in service_unit_files.iter().chain(timer_unit_files.iter()) {
        if !is_systemd_enabled_state(state) {
            continue;
        }
        items.push(json!({
            "name": unit,
            "command": format!("systemctl start {unit}"),
            "location": "systemd",
            "user": "system",
            "is_enabled": true,
        }));
    }
    items.sort_by(|a, b| {
        a.get("name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .cmp(b.get("name").and_then(Value::as_str).unwrap_or_default())
    });
    items
}

fn parse_apt_upgradable_output(stdout: &str, reboot_required: bool) -> Vec<Value> {
    let mut updates = Vec::new();
    let mut seen = HashSet::new();
    for line in stdout.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty()
            || trimmed.starts_with("Listing...")
            || trimmed.starts_with("WARNING:")
            || !trimmed.contains('/')
        {
            continue;
        }
        let columns = trimmed.split_whitespace().collect::<Vec<_>>();
        if columns.len() < 2 {
            continue;
        }
        let package = columns[0].split('/').next().unwrap_or_default().trim();
        let version = columns[1].trim();
        if package.is_empty() || !seen.insert(format!("{package}|{version}")) {
            continue;
        }
        updates.push(json!({
            "title": format!("{package} {version}"),
            "description": trimmed,
            "kb_article": null,
            "is_mandatory": false,
            "size_bytes": null,
            "requires_reboot": reboot_required,
        }));
    }
    updates
}

fn parse_dnf_yum_check_update_output(stdout: &str, reboot_required: bool) -> Vec<Value> {
    let mut updates = Vec::new();
    let mut seen = HashSet::new();
    for line in stdout.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty()
            || trimmed.starts_with("Last metadata expiration check")
            || trimmed.starts_with("Loaded plugins:")
            || trimmed.eq_ignore_ascii_case("Obsoleting Packages")
        {
            continue;
        }
        let columns = trimmed.split_whitespace().collect::<Vec<_>>();
        if columns.len() < 3 {
            continue;
        }
        let package = strip_rpm_arch_suffix(columns[0]);
        let version = columns[1].trim();
        if package.is_empty() || version.is_empty() || !seen.insert(format!("{package}|{version}"))
        {
            continue;
        }
        updates.push(json!({
            "title": format!("{package} {version}"),
            "description": trimmed,
            "kb_article": null,
            "is_mandatory": false,
            "size_bytes": null,
            "requires_reboot": reboot_required,
        }));
    }
    updates
}

fn strip_rpm_arch_suffix(package: &str) -> &str {
    const RPM_ARCH_SUFFIXES: &[&str] = &[
        ".x86_64", ".noarch", ".aarch64", ".i686", ".armv7hl", ".ppc64le", ".s390x",
    ];

    RPM_ARCH_SUFFIXES
        .iter()
        .find_map(|suffix| package.strip_suffix(suffix))
        .unwrap_or(package)
}

fn parse_pacman_updates_output(stdout: &str, reboot_required: bool) -> Vec<Value> {
    let mut updates = Vec::new();
    let mut seen = HashSet::new();
    for line in stdout.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with("warning:") || !trimmed.contains(" -> ") {
            continue;
        }
        let Some((left, new_version)) = trimmed.split_once(" -> ") else {
            continue;
        };
        let Some((package, _old_version)) = left.rsplit_once(' ') else {
            continue;
        };
        let package = package.trim();
        let new_version = new_version.trim();
        if package.is_empty()
            || new_version.is_empty()
            || package.contains(char::is_whitespace)
            || !seen.insert(format!("{package}|{new_version}"))
        {
            continue;
        }
        updates.push(json!({
            "title": format!("{package} {new_version}"),
            "description": trimmed,
            "kb_article": null,
            "is_mandatory": false,
            "size_bytes": null,
            "requires_reboot": reboot_required,
        }));
    }
    updates
}

fn parse_apt_history_output(contents: &str, limit: usize) -> Vec<Value> {
    if limit == 0 {
        return Vec::new();
    }

    let mut entries = Vec::new();
    let mut current_date: Option<String> = None;
    for line in contents.lines() {
        if let Some(raw_date) = line.strip_prefix("Start-Date:") {
            current_date = parse_apt_history_date(raw_date.trim());
            continue;
        }
        if let Some(raw_items) = line.strip_prefix("Install:") {
            push_apt_history_items(
                &mut entries,
                current_date.as_deref(),
                "Installation",
                raw_items,
            );
            continue;
        }
        if let Some(raw_items) = line.strip_prefix("Upgrade:") {
            push_apt_history_items(&mut entries, current_date.as_deref(), "Upgrade", raw_items);
        }
    }

    if entries.len() > limit {
        entries = entries[entries.len() - limit..].to_vec();
    }
    entries.reverse();
    entries
}

fn push_apt_history_items(
    entries: &mut Vec<Value>,
    date: Option<&str>,
    operation: &str,
    raw_items: &str,
) {
    for raw_item in raw_items.split("),") {
        let item = raw_item.trim().trim_end_matches(')').trim();
        if item.is_empty() {
            continue;
        }
        let name = item
            .split_whitespace()
            .next()
            .unwrap_or_default()
            .trim_end_matches(':');
        if name.is_empty() {
            continue;
        }
        let version = item
            .split_once('(')
            .and_then(|(_, rest)| rest.split(',').next())
            .map(str::trim)
            .filter(|value| !value.is_empty());
        let title = match version {
            Some(version) => format!("{name} {version}"),
            None => name.to_string(),
        };
        entries.push(json!({
            "date": date,
            "title": title,
            "operation": operation,
            "result": "Succeeded",
            "kb_article": null,
            "hresult": null,
        }));
    }
}

fn parse_apt_history_date(value: &str) -> Option<String> {
    NaiveDateTime::parse_from_str(value, "%Y-%m-%d  %H:%M:%S")
        .or_else(|_| NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S"))
        .ok()
        .map(|dt| DateTime::<Utc>::from_naive_utc_and_offset(dt, Utc).to_rfc3339())
}

fn package_is_64_bit(name: &str) -> bool {
    if let Some((_, arch)) = name.rsplit_once(':') {
        return package_arch_is_64_bit(arch);
    }
    package_arch_is_64_bit(std::env::consts::ARCH)
}

fn package_arch_is_64_bit(arch: &str) -> bool {
    matches!(arch, "amd64" | "x86_64" | "arm64" | "aarch64")
}

fn service_status(active: &str, sub: &str) -> &'static str {
    match (active, sub) {
        ("active", "running") => "running",
        ("active", _) => "active",
        ("failed", _) => "failed",
        ("inactive", _) => "inactive",
        _ => "unknown",
    }
}

fn is_systemd_enabled_state(state: &str) -> bool {
    matches!(
        state,
        "enabled" | "enabled-runtime" | "linked" | "linked-runtime"
    )
}

fn is_critical_linux_service(unit: &str) -> bool {
    matches!(
        unit,
        "systemd.service"
            | "dbus.service"
            | "ssh.service"
            | "sshd.service"
            | "NetworkManager.service"
            | "systemd-networkd.service"
            | "systemd-resolved.service"
            | "cron.service"
            | "rsyslog.service"
    )
}

fn linux_reboot_required() -> bool {
    fs::metadata("/var/run/reboot-required").is_ok()
}

fn apt_available() -> bool {
    command_available("apt")
}

fn apt_get_available() -> bool {
    command_available("apt-get")
}

fn pacman_available() -> bool {
    command_available("pacman")
}

fn dnf_available() -> bool {
    command_available("dnf")
}

fn yum_available() -> bool {
    command_available("yum")
}

fn package_manager_available() -> bool {
    apt_available() || dnf_available() || yum_available() || pacman_available()
}

fn command_available(command: &str) -> bool {
    std::env::var_os("PATH")
        .and_then(|paths| {
            std::env::split_paths(&paths).find(|path| {
                let command_path = path.join(command);
                command_path.is_file()
            })
        })
        .is_some()
}

#[cfg(unix)]
fn running_as_root() -> bool {
    unsafe { libc::geteuid() == 0 }
}

#[cfg(not(unix))]
fn running_as_root() -> bool {
    false
}

fn apt_automatic_updates_enabled() -> bool {
    for path in [
        "/etc/apt/apt.conf.d/20auto-upgrades",
        "/etc/apt/apt.conf.d/50unattended-upgrades",
    ] {
        let Ok(contents) = fs::read_to_string(path) else {
            continue;
        };
        if contents.contains("Unattended-Upgrade \"1\"")
            || contents.contains("Update-Package-Lists \"1\"")
        {
            return true;
        }
    }
    false
}

fn latest_history_date(update_history: &[Value]) -> Option<String> {
    update_history
        .iter()
        .filter_map(|entry| entry.get("date").and_then(Value::as_str))
        .max()
        .map(ToOwned::to_owned)
}

fn read_string(map: &Map<String, Value>, key: &str) -> Option<String> {
    map.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn read_u64(map: &Map<String, Value>, key: &str) -> Option<u64> {
    map.get(key).and_then(|value| {
        value
            .as_u64()
            .or_else(|| value.as_i64().and_then(|v| v.try_into().ok()))
    })
}

fn epoch_to_rfc3339(epoch: u64) -> Option<String> {
    DateTime::<Utc>::from_timestamp(epoch.try_into().ok()?, 0).map(|dt| dt.to_rfc3339())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_dpkg_query_rows() {
        let rows = parse_dpkg_query_output(
            "bash\t5.2.21-2ubuntu4\tUbuntu Developers\t1420\tii \nremoved\t1.0\tNobody\t1\trc \n",
        );
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["name"], "bash");
        assert_eq!(rows[0]["size_bytes"], 1_454_080_u64);
    }

    #[test]
    fn parses_rpm_query_rows() {
        let rows = parse_rpm_query_output(
            "libgcc\t15.2.1-2.fc43\tFedora Project\t272996\tx86_64\t1779456170\nwhois-nls\t5.6.4-1.fc43\tFedora Project\t135417\tnoarch\t1779456171\n",
        );
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["name"], "libgcc");
        assert_eq!(rows[0]["publisher"], "Fedora Project");
        assert_eq!(rows[0]["source"], "rpm");
        assert_eq!(rows[0]["size_bytes"], 272_996_u64);
        assert_eq!(rows[0]["architecture"], "x86_64");
        assert_eq!(rows[0]["is_64_bit"], true);
        assert_eq!(rows[0]["uninstall_string"], "dnf remove libgcc");
        assert_eq!(rows[1]["is_64_bit"], false);
    }

    #[test]
    fn parses_systemd_services_and_startup_items() {
        let unit_files = parse_systemd_unit_files(
            "ssh.service enabled enabled\ncron.service disabled enabled\n",
        );
        let services = parse_systemd_services(
            "ssh.service loaded active running OpenBSD Secure Shell server\ncron.service loaded inactive dead Regular background program processing daemon\n",
            &unit_files,
        );
        assert_eq!(services.len(), 2);
        assert_eq!(services[0]["name"], "cron.service");
        assert_eq!(services[1]["status"], "running");

        let timers = parse_systemd_unit_files("apt-daily.timer enabled enabled\n");
        let startup = startup_items_from_unit_files(&unit_files, &timers);
        assert_eq!(startup.len(), 2);
    }

    #[test]
    fn parses_apt_upgradable_output() {
        let updates = parse_apt_upgradable_output(
            "Listing... Done\nopenssl/noble-updates 3.0.13-0ubuntu3.6 amd64 [upgradable from: 3.0.13-0ubuntu3.5]\n",
            true,
        );
        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0]["title"], "openssl 3.0.13-0ubuntu3.6");
        assert_eq!(updates[0]["requires_reboot"], true);
    }

    #[test]
    fn apt_metadata_refreshes_when_no_cache_exists() {
        assert!(apt_metadata_refresh_required_from_mtime(
            None,
            SystemTime::UNIX_EPOCH + Duration::from_secs(1)
        ));
    }

    #[test]
    fn apt_metadata_cache_respects_max_age() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(10_000);
        assert!(!apt_metadata_refresh_required_from_mtime(
            Some(now - Duration::from_secs(60)),
            now
        ));
        assert!(apt_metadata_refresh_required_from_mtime(
            Some(now - APT_METADATA_CACHE_MAX_AGE),
            now
        ));
    }

    #[test]
    fn parses_dnf_yum_check_update_output() {
        let updates = parse_dnf_yum_check_update_output(
            "NetworkManager.x86_64 1:1.46.0-1.fc40 updates\n",
            true,
        );

        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0]["title"], "NetworkManager 1:1.46.0-1.fc40");
        assert_eq!(
            updates[0]["description"],
            "NetworkManager.x86_64 1:1.46.0-1.fc40 updates"
        );
        assert_eq!(updates[0]["kb_article"], Value::Null);
        assert_eq!(updates[0]["is_mandatory"], false);
        assert_eq!(updates[0]["size_bytes"], Value::Null);
        assert_eq!(updates[0]["requires_reboot"], true);
    }

    #[test]
    fn parses_dnf_yum_check_update_output_dedupes() {
        let updates = parse_dnf_yum_check_update_output(
            "NetworkManager.x86_64 1:1.46.0-1.fc40 updates\nkernel.x86_64 6.8.9-300.fc40 updates\nNetworkManager.x86_64 1:1.46.0-1.fc40 updates\n",
            false,
        );

        assert_eq!(updates.len(), 2);
        assert_eq!(updates[0]["title"], "NetworkManager 1:1.46.0-1.fc40");
        assert_eq!(updates[1]["title"], "kernel 6.8.9-300.fc40");
        assert_eq!(updates[0]["requires_reboot"], false);
    }

    #[test]
    fn parses_dnf_yum_check_update_output_strips_rpm_arch_suffixes() {
        let updates = parse_dnf_yum_check_update_output(
            "pkg-x86.x86_64 1 updates\npkg-no.noarch 1 updates\npkg-arm.aarch64 1 updates\npkg-i686.i686 1 updates\npkg-armv7.armv7hl 1 updates\npkg-ppc.ppc64le 1 updates\npkg-s390.s390x 1 updates\npkg-raw.riscv64 1 updates\n",
            false,
        );

        let titles = updates
            .iter()
            .filter_map(|update| update.get("title").and_then(Value::as_str))
            .collect::<Vec<_>>();
        assert_eq!(
            titles,
            vec![
                "pkg-x86 1",
                "pkg-no 1",
                "pkg-arm 1",
                "pkg-i686 1",
                "pkg-armv7 1",
                "pkg-ppc 1",
                "pkg-s390 1",
                "pkg-raw.riscv64 1",
            ]
        );
    }

    #[test]
    fn parses_dnf_yum_check_update_output_ignores_noise() {
        let updates = parse_dnf_yum_check_update_output(
            "\nLast metadata expiration check: 0:01:23 ago on Tue 19 May 2026 10:00:00 AM UTC.\nLoaded plugins: fastestmirror\nObsoleting Packages\nmalformed\nmissing-version.x86_64 updates\nvalid.noarch 2.0-1.el9 baseos\n",
            false,
        );

        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0]["title"], "valid 2.0-1.el9");
    }

    #[test]
    fn parses_pacman_updates_output() {
        let updates = parse_pacman_updates_output("linux 6.8.1.arch1-1 -> 6.8.2.arch1-1\n", true);

        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0]["title"], "linux 6.8.2.arch1-1");
        assert_eq!(
            updates[0]["description"],
            "linux 6.8.1.arch1-1 -> 6.8.2.arch1-1"
        );
        assert_eq!(updates[0]["kb_article"], Value::Null);
        assert_eq!(updates[0]["is_mandatory"], false);
        assert_eq!(updates[0]["size_bytes"], Value::Null);
        assert_eq!(updates[0]["requires_reboot"], true);
    }

    #[test]
    fn parses_pacman_updates_output_dedupes() {
        let updates = parse_pacman_updates_output(
            "linux 6.8.1.arch1-1 -> 6.8.2.arch1-1\nopenssl 3.2.1-1 -> 3.2.2-1\nlinux 6.8.1.arch1-1 -> 6.8.2.arch1-1\n",
            false,
        );

        assert_eq!(updates.len(), 2);
        assert_eq!(updates[0]["title"], "linux 6.8.2.arch1-1");
        assert_eq!(updates[1]["title"], "openssl 3.2.2-1");
        assert_eq!(updates[0]["requires_reboot"], false);
    }

    #[test]
    fn parses_pacman_updates_output_ignores_invalid_lines() {
        let updates = parse_pacman_updates_output(
            "\nwarning: database file for 'core' does not exist\nnot-an-update\nmissing-new-version 1.0 -> \nmissing-old-version -> 2.0\nbad package 1.0 -> 2.0\npacman 6.1.0-3 -> 6.1.0-4\n",
            false,
        );

        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0]["title"], "pacman 6.1.0-4");
    }

    #[test]
    fn parses_apt_history_output() {
        let history = parse_apt_history_output(
            "Start-Date: 2026-05-16  10:00:00\nInstall: curl:amd64 (8.5.0-2ubuntu10)\nUpgrade: openssl:amd64 (3.0.13-0ubuntu3.6, 3.0.13-0ubuntu3.7)\nEnd-Date: 2026-05-16  10:00:01\n",
            10,
        );
        assert_eq!(history.len(), 2);
        assert_eq!(history[0]["operation"], "Upgrade");
        assert_eq!(history[1]["operation"], "Installation");
    }
}
