use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    sync::Arc,
    time::Duration,
};

use serde_json::{json, Value};
use sysinfo::{Disks, System};
use tokio::{
    sync::{broadcast, mpsc},
    time::sleep,
};
use tracing::{info, warn};

use crate::{macos_telemetry, LiveEventInput};

#[derive(Debug, Clone, Copy)]
struct MacosEventMonitorConfig {
    services_interval: Duration,
    software_interval: Duration,
    network_interval: Duration,
    system_interval: Duration,
    updates_interval: Duration,
    disk_interval: Duration,
    boot_event_max_start_delay_secs: u64,
}

impl Default for MacosEventMonitorConfig {
    fn default() -> Self {
        Self {
            services_interval: Duration::from_secs(45),
            software_interval: Duration::from_secs(120),
            network_interval: Duration::from_secs(45),
            system_interval: Duration::from_secs(120),
            updates_interval: Duration::from_secs(180),
            disk_interval: Duration::from_secs(300),
            boot_event_max_start_delay_secs: 600,
        }
    }
}

pub(crate) fn start_macos_event_stream_bridge(
    boot_session_id: String,
    live_events_tx: broadcast::Sender<Value>,
    live_event_backlog: Arc<tokio::sync::Mutex<VecDeque<Value>>>,
) {
    tokio::spawn(async move {
        let (tx, mut rx) = mpsc::channel::<LiveEventInput>(2048);
        spawn_macos_monitors(tx, MacosEventMonitorConfig::default(), boot_session_id);
        info!("macOS live telemetry event stream started");

        while let Some(event) = rx.recv().await {
            crate::publish_live_event(
                &live_events_tx,
                &live_event_backlog,
                crate::normalize_live_event(event),
            )
            .await;
        }
        warn!("macOS live telemetry monitor stream stopped");
    });
}

fn spawn_macos_monitors(
    sender: mpsc::Sender<LiveEventInput>,
    config: MacosEventMonitorConfig,
    boot_session_id: String,
) {
    tokio::spawn(run_system_monitor(sender.clone(), config, boot_session_id));
    tokio::spawn(run_services_monitor(sender.clone(), config));
    tokio::spawn(run_software_monitor(sender.clone(), config));
    tokio::spawn(run_network_monitor(sender.clone(), config));
    tokio::spawn(run_updates_monitor(sender.clone(), config));
    tokio::spawn(run_disk_monitor(sender, config));
}

async fn run_system_monitor(
    sender: mpsc::Sender<LiveEventInput>,
    config: MacosEventMonitorConfig,
    boot_session_id: String,
) {
    if should_emit_boot_event(config.boot_event_max_start_delay_secs) {
        let _ = sender
            .send(LiveEventInput::new(
                "system",
                "boot",
                format!("system:{boot_session_id}"),
                json!({ "boot_session_id": boot_session_id }),
            ))
            .await;
    }

    let mut previous_hostname: Option<String> = None;
    loop {
        let current = current_hostname();
        if let Some(previous) = previous_hostname.as_ref() {
            if previous != &current {
                let _ = sender
                    .send(LiveEventInput::new(
                        "system",
                        "hostname_changed",
                        "system:hostname",
                        json!({
                            "previous_hostname": previous,
                            "hostname": current,
                        }),
                    ))
                    .await;
            }
        }
        previous_hostname = Some(current);
        sleep(config.system_interval).await;
    }
}

fn should_emit_boot_event(max_start_delay_secs: u64) -> bool {
    let boot_time = System::boot_time();
    if boot_time == 0 {
        return false;
    }

    let now_secs = chrono::Utc::now().timestamp();
    let delay_secs = now_secs.saturating_sub(boot_time as i64) as u64;
    delay_secs <= max_start_delay_secs
}

fn current_hostname() -> String {
    hostname::get()
        .ok()
        .and_then(|name| name.into_string().ok())
        .unwrap_or_else(|| "unknown".to_string())
}

async fn run_services_monitor(
    sender: mpsc::Sender<LiveEventInput>,
    config: MacosEventMonitorConfig,
) {
    let mut previous = BTreeMap::new();
    let mut initialized = false;
    loop {
        let (services, _) = macos_telemetry::collect_services_and_startup_items().await;
        let current = service_snapshot_map(&services);
        if initialized {
            send_events(&sender, diff_service_snapshots(&previous, &current)).await;
        }
        previous = current;
        initialized = true;
        sleep(config.services_interval).await;
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ServiceSnapshot {
    status: String,
    start_mode: String,
}

fn service_snapshot_map(services: &[Value]) -> BTreeMap<String, ServiceSnapshot> {
    services
        .iter()
        .filter_map(|service| {
            let name = read_string(service, &["name", "service_name", "label"])?;
            Some((
                name,
                ServiceSnapshot {
                    status: read_string(service, &["status"]).unwrap_or_else(|| "unknown".into()),
                    start_mode: service_start_mode(service),
                },
            ))
        })
        .collect()
}

fn service_start_mode(service: &Value) -> String {
    match read_bool(service, &["is_enabled"]) {
        Some(true) => "enabled".to_string(),
        Some(false) => "disabled".to_string(),
        None => read_string(service, &["start_type"]).unwrap_or_else(|| "launchd".to_string()),
    }
}

fn diff_service_snapshots(
    previous: &BTreeMap<String, ServiceSnapshot>,
    current: &BTreeMap<String, ServiceSnapshot>,
) -> Vec<LiveEventInput> {
    let mut events = Vec::new();
    for (name, current_service) in current {
        let Some(previous_service) = previous.get(name) else {
            continue;
        };
        if previous_service.status != current_service.status {
            events.push(LiveEventInput::new(
                "service",
                "state_changed",
                format!("service:{name}"),
                json!({
                    "name": name,
                    "previous_status": previous_service.status,
                    "status": current_service.status,
                }),
            ));
        }
        if previous_service.start_mode != current_service.start_mode {
            events.push(LiveEventInput::new(
                "service",
                "start_type_changed",
                format!("service:{name}"),
                json!({
                    "name": name,
                    "previous_start_type": previous_service.start_mode,
                    "start_type": current_service.start_mode,
                }),
            ));
        }
    }
    events
}

async fn run_software_monitor(
    sender: mpsc::Sender<LiveEventInput>,
    config: MacosEventMonitorConfig,
) {
    let mut previous = BTreeMap::new();
    let mut initialized = false;
    loop {
        let apps = macos_telemetry::collect_installed_programs().await;
        let current = software_snapshot_map(&apps);
        if initialized {
            send_events(&sender, diff_software_snapshots(&previous, &current)).await;
        }
        previous = current;
        initialized = true;
        sleep(config.software_interval).await;
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SoftwareSnapshot {
    name: String,
    version: String,
    publisher: String,
    location: String,
    source: String,
}

fn software_snapshot_map(apps: &[Value]) -> BTreeMap<String, SoftwareSnapshot> {
    let mut map = BTreeMap::new();
    for app in apps {
        let Some(name) = read_string(app, &["name"]) else {
            continue;
        };
        let snapshot = SoftwareSnapshot {
            name,
            version: read_string(app, &["version"]).unwrap_or_default(),
            publisher: read_string(app, &["publisher", "vendor"]).unwrap_or_default(),
            location: read_string(app, &["location", "path"]).unwrap_or_default(),
            source: read_string(app, &["source"]).unwrap_or_default(),
        };
        map.insert(software_identity_key(&snapshot), snapshot);
    }
    map
}

fn software_identity_key(app: &SoftwareSnapshot) -> String {
    let location_or_source = if app.location.trim().is_empty() {
        app.source.trim()
    } else {
        app.location.trim()
    };
    format!(
        "{}::{}::{}",
        normalize_text(&app.name),
        normalize_text(&app.publisher),
        normalize_text(location_or_source),
    )
}

fn software_scope_key(app: &SoftwareSnapshot, identity_key: &str) -> String {
    let name = normalize_text(&app.name);
    let name = if name.is_empty() { "unknown" } else { &name };
    format!("software:{name}:{}", short_identity(identity_key))
}

fn diff_software_snapshots(
    previous: &BTreeMap<String, SoftwareSnapshot>,
    current: &BTreeMap<String, SoftwareSnapshot>,
) -> Vec<LiveEventInput> {
    let mut events = Vec::new();

    for (identity_key, current_app) in current {
        match previous.get(identity_key) {
            None => events.push(LiveEventInput::new(
                "software",
                "installed",
                software_scope_key(current_app, identity_key),
                json!({
                    "name": current_app.name,
                    "version": current_app.version,
                    "publisher": current_app.publisher,
                    "location": current_app.location,
                    "source": current_app.source,
                }),
            )),
            Some(previous_app)
                if previous_app.version != current_app.version
                    || previous_app.publisher != current_app.publisher =>
            {
                events.push(LiveEventInput::new(
                    "software",
                    "updated",
                    software_scope_key(current_app, identity_key),
                    json!({
                        "name": current_app.name,
                        "previous_version": previous_app.version,
                        "version": current_app.version,
                        "previous_publisher": previous_app.publisher,
                        "publisher": current_app.publisher,
                        "location": current_app.location,
                        "source": current_app.source,
                    }),
                ));
            }
            _ => {}
        }
    }

    for (identity_key, previous_app) in previous {
        if current.contains_key(identity_key) {
            continue;
        }
        events.push(LiveEventInput::new(
            "software",
            "uninstalled",
            software_scope_key(previous_app, identity_key),
            json!({
                "name": previous_app.name,
                "version": previous_app.version,
                "publisher": previous_app.publisher,
                "location": previous_app.location,
                "source": previous_app.source,
            }),
        ));
    }

    events
}

async fn run_network_monitor(
    sender: mpsc::Sender<LiveEventInput>,
    config: MacosEventMonitorConfig,
) {
    let mut previous = BTreeMap::new();
    let mut initialized = false;
    loop {
        let adapters = macos_telemetry::collect_network_adapters().await;
        let current = network_snapshot_map(&adapters);
        if initialized {
            send_events(&sender, diff_network_snapshots(&previous, &current)).await;
        }
        previous = current;
        initialized = true;
        sleep(config.network_interval).await;
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct NetworkSnapshot {
    fingerprint: String,
}

fn network_snapshot_map(adapters: &[Value]) -> BTreeMap<String, NetworkSnapshot> {
    let mut map = BTreeMap::new();
    for adapter in adapters {
        let name = read_string(adapter, &["name"]).unwrap_or_default();
        let mac = read_string(adapter, &["mac_address"]).unwrap_or_default();
        let adapter_key = if !name.trim().is_empty() {
            name
        } else if !mac.trim().is_empty() {
            mac
        } else {
            continue;
        };
        map.insert(
            adapter_key,
            NetworkSnapshot {
                fingerprint: network_fingerprint(adapter),
            },
        );
    }
    map
}

fn network_fingerprint(adapter: &Value) -> String {
    let mut ips = adapter
        .get("ips")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|ip| {
                    format!(
                        "{}/{}:{}",
                        read_string(ip, &["address"]).unwrap_or_default(),
                        read_u64(ip, &["prefix"]).unwrap_or(0),
                        read_string(ip, &["family"]).unwrap_or_default(),
                    )
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    ips.sort();

    let mut gateways = sorted_string_values(adapter.get("gateways"));
    let mut dns_servers = sorted_string_values(adapter.get("dns_servers"));
    gateways.sort();
    dns_servers.sort();

    json!({
        "ips": ips,
        "gateways": gateways,
        "dns_servers": dns_servers,
        "dns_suffix": read_string(adapter, &["dns_suffix"]).unwrap_or_default(),
        "mac_address": read_string(adapter, &["mac_address"]).unwrap_or_default(),
        "status": read_string(adapter, &["status"]).unwrap_or_default(),
    })
    .to_string()
}

fn diff_network_snapshots(
    previous: &BTreeMap<String, NetworkSnapshot>,
    current: &BTreeMap<String, NetworkSnapshot>,
) -> Vec<LiveEventInput> {
    let mut events = Vec::new();

    for (adapter_key, current_adapter) in current {
        match previous.get(adapter_key) {
            None => events.push(LiveEventInput::new(
                "network",
                "adapter_added",
                format!("network:{adapter_key}"),
                json!({ "adapter_key": adapter_key }),
            )),
            Some(previous_adapter)
                if previous_adapter.fingerprint != current_adapter.fingerprint =>
            {
                events.push(LiveEventInput::new(
                    "network",
                    "identity_changed",
                    format!("network:{adapter_key}"),
                    json!({ "adapter_key": adapter_key }),
                ));
            }
            _ => {}
        }
    }

    for adapter_key in previous.keys() {
        if current.contains_key(adapter_key) {
            continue;
        }
        events.push(LiveEventInput::new(
            "network",
            "adapter_removed",
            format!("network:{adapter_key}"),
            json!({ "adapter_key": adapter_key }),
        ));
    }

    events
}

async fn run_updates_monitor(
    sender: mpsc::Sender<LiveEventInput>,
    config: MacosEventMonitorConfig,
) {
    let mut previous: Option<UpdatesSnapshot> = None;
    loop {
        let current = collect_updates_snapshot().await;
        if let Some(previous) = previous.as_ref() {
            send_events(&sender, diff_update_snapshots(previous, &current)).await;
        }
        previous = Some(current);
        sleep(config.updates_interval).await;
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct UpdatesSnapshot {
    reboot_pending: bool,
    history: BTreeMap<String, UpdateHistoryItem>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct UpdateHistoryItem {
    title: String,
    version: String,
    installed_at: String,
    operation: String,
    result: String,
    update_key: String,
}

async fn collect_updates_snapshot() -> UpdatesSnapshot {
    let pending_updates = macos_telemetry::collect_pending_updates().await;
    let reboot_pending = macos_telemetry::macos_reboot_required(&pending_updates);
    let update_history = macos_telemetry::collect_update_history().await;

    UpdatesSnapshot {
        reboot_pending,
        history: update_history_map(&update_history),
    }
}

fn update_history_map(history: &[Value]) -> BTreeMap<String, UpdateHistoryItem> {
    let mut map = BTreeMap::new();
    for entry in history {
        let Some(title) = read_string(entry, &["title", "name"]) else {
            continue;
        };
        let item = UpdateHistoryItem {
            title,
            version: read_string(entry, &["version"]).unwrap_or_default(),
            installed_at: read_string(entry, &["installed_at", "installedAt", "date"])
                .unwrap_or_default(),
            operation: read_string(entry, &["operation"]).unwrap_or_else(|| "install".into()),
            result: read_string(entry, &["result", "status"]).unwrap_or_else(|| "succeeded".into()),
            update_key: read_string(entry, &["update_key", "updateKey"]).unwrap_or_default(),
        };
        map.insert(update_history_identity(&item), item);
    }
    map
}

fn update_history_identity(item: &UpdateHistoryItem) -> String {
    format!(
        "{}::{}::{}",
        normalize_text(&item.title),
        normalize_text(&item.version),
        item.installed_at,
    )
}

fn update_scope_key(item: &UpdateHistoryItem) -> String {
    let key = if item.update_key.trim().is_empty() {
        item.title.trim()
    } else {
        item.update_key.trim()
    };
    format!("updates:{key}")
}

fn diff_update_snapshots(
    previous: &UpdatesSnapshot,
    current: &UpdatesSnapshot,
) -> Vec<LiveEventInput> {
    let mut events = Vec::new();

    if previous.reboot_pending != current.reboot_pending {
        events.push(LiveEventInput::new(
            "updates",
            "reboot_pending",
            "updates:reboot",
            json!({ "reboot_pending": current.reboot_pending }),
        ));
    }

    for (identity_key, item) in &current.history {
        if previous.history.contains_key(identity_key) {
            continue;
        }
        events.push(LiveEventInput::new(
            "updates",
            "install_completed",
            update_scope_key(item),
            json!({
                "title": item.title,
                "version": item.version,
                "installed_at": item.installed_at,
                "operation": item.operation,
                "result": item.result,
                "update_key": item.update_key,
            }),
        ));
    }

    events
}

async fn run_disk_monitor(sender: mpsc::Sender<LiveEventInput>, config: MacosEventMonitorConfig) {
    let mut active_alerts = BTreeSet::new();
    let mut initialized = false;
    loop {
        let disks = collect_disk_statuses();
        let events = diff_disk_statuses(&mut active_alerts, &disks, initialized);
        send_events(&sender, events).await;
        initialized = true;
        sleep(config.disk_interval).await;
    }
}

#[derive(Debug, Clone, PartialEq)]
struct DiskStatus {
    mount_point: String,
    available_bytes: u64,
    total_bytes: u64,
}

fn collect_disk_statuses() -> Vec<DiskStatus> {
    let mut disks = Disks::new_with_refreshed_list();
    disks.refresh();
    disks
        .list()
        .iter()
        .filter_map(|disk| {
            let total_bytes = disk.total_space();
            if total_bytes == 0 {
                return None;
            }
            Some(DiskStatus {
                mount_point: disk.mount_point().to_string_lossy().to_string(),
                available_bytes: disk.available_space(),
                total_bytes,
            })
        })
        .collect()
}

fn diff_disk_statuses(
    active_alerts: &mut BTreeSet<String>,
    disks: &[DiskStatus],
    initialized: bool,
) -> Vec<LiveEventInput> {
    let mut events = Vec::new();
    let mut seen_mounts = BTreeSet::new();

    for disk in disks {
        seen_mounts.insert(disk.mount_point.clone());
        let free_percent = if disk.total_bytes > 0 {
            (disk.available_bytes as f64 / disk.total_bytes as f64) * 100.0
        } else {
            100.0
        };
        let in_low_space = free_percent < 10.0;

        if !initialized {
            if in_low_space {
                active_alerts.insert(disk.mount_point.clone());
            }
            continue;
        }

        if in_low_space && active_alerts.insert(disk.mount_point.clone()) {
            events.push(LiveEventInput::new(
                "disk",
                "low_space",
                format!("disk:{}", disk.mount_point),
                json!({
                    "mount_point": disk.mount_point,
                    "free_percent": free_percent,
                    "available_bytes": disk.available_bytes,
                    "total_bytes": disk.total_bytes,
                }),
            ));
        } else if !in_low_space {
            active_alerts.remove(&disk.mount_point);
        }
    }

    active_alerts.retain(|mount| seen_mounts.contains(mount));
    events
}

async fn send_events(sender: &mpsc::Sender<LiveEventInput>, events: Vec<LiveEventInput>) {
    for event in events {
        if sender.send(event).await.is_err() {
            break;
        }
    }
}

fn read_string(value: &Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| text_value(value.get(*key)))
        .filter(|value| !value.trim().is_empty())
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

fn read_bool(value: &Value, keys: &[&str]) -> Option<bool> {
    keys.iter().find_map(|key| {
        value.get(*key).and_then(|value| match value {
            Value::Bool(value) => Some(*value),
            Value::String(value) => match value.trim().to_ascii_lowercase().as_str() {
                "1" | "yes" | "true" | "enabled" => Some(true),
                "0" | "no" | "false" | "disabled" => Some(false),
                _ => None,
            },
            Value::Number(value) => value.as_i64().map(|value| value != 0),
            _ => None,
        })
    })
}

fn read_u64(value: &Value, keys: &[&str]) -> Option<u64> {
    keys.iter().find_map(|key| {
        value
            .get(*key)
            .and_then(|value| value.as_u64().or_else(|| value.as_str()?.parse().ok()))
    })
}

fn sorted_string_values(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| text_value(Some(item)))
                .collect()
        })
        .unwrap_or_default()
}

fn normalize_text(value: &str) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase()
}

fn short_identity(identity_key: &str) -> String {
    let value = identity_key
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .take(12)
        .collect::<String>();
    if value.is_empty() {
        "unknown".to_string()
    } else {
        value
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_diff_detects_state_and_enabled_changes() {
        let previous = service_snapshot_map(&[json!({
            "name": "com.example.agent",
            "status": "running",
            "is_enabled": true,
        })]);
        let current = service_snapshot_map(&[json!({
            "name": "com.example.agent",
            "status": "stopped",
            "is_enabled": false,
        })]);

        let events = diff_service_snapshots(&previous, &current);
        let kinds = event_kinds(&events);

        assert_eq!(kinds, vec!["state_changed", "start_type_changed"]);
        assert_eq!(events[0].event_type, "service");
        assert_eq!(events[0].scope_key, "service:com.example.agent");
    }

    #[test]
    fn software_diff_detects_install_update_and_uninstall() {
        let previous = software_snapshot_map(&[
            json!({
                "name": "Example App",
                "version": "1.0",
                "publisher": "Example",
                "location": "/Applications/Example.app",
                "source": "system_profiler",
            }),
            json!({
                "name": "Old App",
                "version": "1.0",
                "publisher": "Example",
                "location": "/Applications/Old.app",
            }),
        ]);
        let current = software_snapshot_map(&[
            json!({
                "name": "Example App",
                "version": "2.0",
                "publisher": "Example",
                "location": "/Applications/Example.app",
                "source": "system_profiler",
            }),
            json!({
                "name": "New App",
                "version": "1.0",
                "publisher": "Example",
                "location": "/Applications/New.app",
            }),
        ]);

        let events = diff_software_snapshots(&previous, &current);
        let kinds = event_kinds(&events);

        assert_eq!(kinds, vec!["updated", "installed", "uninstalled"]);
        assert!(events
            .iter()
            .any(|event| event.scope_key.starts_with("software:example app:")));
    }

    #[test]
    fn network_diff_detects_added_removed_and_changed_adapters() {
        let previous = network_snapshot_map(&[
            json!({
                "name": "en0",
                "mac_address": "aa:bb:cc:dd:ee:ff",
                "ips": [{ "address": "192.168.1.20", "family": "ipv4", "prefix": 24 }],
                "gateways": ["192.168.1.1"],
                "dns_servers": ["1.1.1.1"],
                "status": "up",
            }),
            json!({ "name": "bridge0", "mac_address": "11:22:33:44:55:66" }),
        ]);
        let current = network_snapshot_map(&[
            json!({
                "name": "en0",
                "mac_address": "aa:bb:cc:dd:ee:ff",
                "ips": [{ "address": "192.168.1.21", "family": "ipv4", "prefix": 24 }],
                "gateways": ["192.168.1.1"],
                "dns_servers": ["1.1.1.1"],
                "status": "up",
            }),
            json!({ "name": "en1", "mac_address": "22:33:44:55:66:77" }),
        ]);

        let events = diff_network_snapshots(&previous, &current);
        let kinds = event_kinds(&events);

        assert_eq!(
            kinds,
            vec!["identity_changed", "adapter_added", "adapter_removed"]
        );
        assert_eq!(events[0].scope_key, "network:en0");
    }

    #[test]
    fn update_diff_detects_reboot_pending_and_new_install_history() {
        let previous = UpdatesSnapshot {
            reboot_pending: false,
            history: update_history_map(&[json!({
                "title": "macOS 14.4",
                "version": "14.4",
                "installed_at": "2024-04-01T10:00:00Z",
                "result": "succeeded",
                "update_key": "macos 14.4|",
            })]),
        };
        let current = UpdatesSnapshot {
            reboot_pending: true,
            history: update_history_map(&[
                json!({
                    "title": "macOS 14.5",
                    "version": "14.5",
                    "installed_at": "2024-05-01T10:00:00Z",
                    "result": "succeeded",
                    "update_key": "macos 14.5|",
                }),
                json!({
                    "title": "macOS 14.4",
                    "version": "14.4",
                    "installed_at": "2024-04-01T10:00:00Z",
                    "result": "succeeded",
                    "update_key": "macos 14.4|",
                }),
            ]),
        };

        let events = diff_update_snapshots(&previous, &current);
        let kinds = event_kinds(&events);

        assert_eq!(kinds, vec!["reboot_pending", "install_completed"]);
        assert_eq!(events[1].scope_key, "updates:macos 14.5|");
    }

    #[test]
    fn disk_diff_emits_only_when_crossing_into_low_space_after_baseline() {
        let mut active_alerts = BTreeSet::new();
        let low = DiskStatus {
            mount_point: "/".to_string(),
            available_bytes: 5,
            total_bytes: 100,
        };
        let healthy = DiskStatus {
            mount_point: "/".to_string(),
            available_bytes: 50,
            total_bytes: 100,
        };

        let events = diff_disk_statuses(&mut active_alerts, std::slice::from_ref(&low), false);
        assert!(events.is_empty());
        assert!(active_alerts.contains("/"));

        let events = diff_disk_statuses(&mut active_alerts, &[healthy], true);
        assert!(events.is_empty());
        assert!(!active_alerts.contains("/"));

        let events = diff_disk_statuses(&mut active_alerts, &[low], true);
        assert_eq!(event_kinds(&events), vec!["low_space"]);
        assert!(active_alerts.contains("/"));
    }

    #[test]
    fn normalization_maps_macos_event_severity() {
        let critical = crate::normalize_live_event(LiveEventInput::new(
            "updates",
            "install_failed",
            "updates:test",
            json!({}),
        ));
        let warning = crate::normalize_live_event(LiveEventInput::new(
            "disk",
            "low_space",
            "disk:/",
            json!({}),
        ));
        let info = crate::normalize_live_event(LiveEventInput::new(
            "service",
            "state_changed",
            "service:test",
            json!({}),
        ));

        assert_eq!(critical["severity"], "error");
        assert_eq!(warning["severity"], "warning");
        assert_eq!(info["severity"], "info");
        assert_eq!(info["source"], "agent_event_stream");
        assert_eq!(info["attributes"]["scopeKey"], "service:test");
    }

    #[tokio::test]
    async fn publish_live_event_backlogs_only_without_receivers() {
        let (live_events_tx, initial_rx) = broadcast::channel::<Value>(4);
        drop(initial_rx);
        let backlog = Arc::new(tokio::sync::Mutex::new(VecDeque::new()));

        crate::publish_live_event(
            &live_events_tx,
            &backlog,
            json!({ "eventType": "system", "code": "boot" }),
        )
        .await;
        assert_eq!(backlog.lock().await.len(), 1);

        let mut live_events_rx = live_events_tx.subscribe();
        crate::publish_live_event(
            &live_events_tx,
            &backlog,
            json!({ "eventType": "service", "code": "state_changed" }),
        )
        .await;

        assert_eq!(backlog.lock().await.len(), 1);
        let received = live_events_rx.recv().await.expect("broadcast event");
        assert_eq!(received["eventType"], "service");
    }

    fn event_kinds(events: &[LiveEventInput]) -> Vec<&str> {
        events
            .iter()
            .map(|event| event.event_kind.as_str())
            .collect()
    }
}
