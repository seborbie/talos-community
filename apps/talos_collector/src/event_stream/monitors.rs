use crate::collectors::{
    certificates::CertificatesCollector, events::EventsCollector, network::NetworkCollector,
    scheduled_tasks::ScheduledTasksCollector, security::SecurityCollector,
    services::ServicesCollector, sessions::SessionsCollector, software::SoftwareCollector,
    system::SystemCollector, updates::UpdatesCollector, Collector,
};
use crate::event_stream::schema::EventInput;
use crate::models::{
    CertificatesInfo, EventsSummary, InstalledProgram, NetworkInfo, ScheduledTasksInfo,
    SecurityInfo, ServicesInfo, SessionsInfo, SoftwareInfo, SystemInfo, UpdatesInfo,
};
use serde::de::DeserializeOwned;
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::time::Duration;
use sysinfo::{Disks, System};
use tokio::sync::mpsc::Sender;
use tracing::warn;

#[derive(Debug, Clone)]
pub struct MonitorConfig {
    pub services_interval: Duration,
    pub software_interval: Duration,
    pub network_interval: Duration,
    pub system_interval: Duration,
    pub updates_interval: Duration,
    pub scheduled_tasks_interval: Duration,
    pub sessions_interval: Duration,
    pub security_interval: Duration,
    pub disk_interval: Duration,
    pub eventlog_interval: Duration,
    pub certificates_interval: Duration,
    pub cert_expiring_days: i64,
    pub boot_event_max_start_delay_secs: u64,
}

impl Default for MonitorConfig {
    fn default() -> Self {
        Self {
            services_interval: Duration::from_secs(45),
            software_interval: Duration::from_secs(120),
            network_interval: Duration::from_secs(45),
            system_interval: Duration::from_secs(120),
            updates_interval: Duration::from_secs(180),
            scheduled_tasks_interval: Duration::from_secs(120),
            sessions_interval: Duration::from_secs(60),
            security_interval: Duration::from_secs(120),
            disk_interval: Duration::from_secs(300),
            eventlog_interval: Duration::from_secs(120),
            certificates_interval: Duration::from_secs(3600),
            cert_expiring_days: 30,
            boot_event_max_start_delay_secs: 600,
        }
    }
}

pub fn spawn_monitors(sender: Sender<EventInput>, config: MonitorConfig, boot_session_id: String) {
    tokio::spawn(run_services_monitor(sender.clone(), config.clone()));
    tokio::spawn(run_software_monitor(sender.clone(), config.clone()));
    tokio::spawn(run_network_monitor(sender.clone(), config.clone()));
    tokio::spawn(run_system_monitor(
        sender.clone(),
        config.clone(),
        boot_session_id,
    ));
    tokio::spawn(run_updates_monitor(sender.clone(), config.clone()));
    tokio::spawn(run_scheduled_tasks_monitor(sender.clone(), config.clone()));
    tokio::spawn(run_sessions_monitor(sender.clone(), config.clone()));
    tokio::spawn(run_security_monitor(sender.clone(), config.clone()));
    tokio::spawn(run_disk_monitor(sender.clone(), config.clone()));
    tokio::spawn(run_eventlog_monitor(sender.clone(), config.clone()));
    tokio::spawn(run_certificate_monitor(sender, config));
}

async fn run_services_monitor(sender: Sender<EventInput>, config: MonitorConfig) {
    let collector = ServicesCollector;
    let mut previous: BTreeMap<String, (String, String)> = BTreeMap::new();
    loop {
        if let Some(current) = collect_typed::<_, ServicesInfo>(&collector).await {
            let current_map: BTreeMap<String, (String, String)> = current
                .services
                .iter()
                .map(|s| (s.name.clone(), (s.status.clone(), s.start_type.clone())))
                .collect();
            for (name, (status, start_type)) in &current_map {
                if let Some((prev_status, prev_start_type)) = previous.get(name) {
                    if prev_status != status {
                        let _ = sender
                            .send(EventInput::new(
                                "service",
                                "state_changed",
                                format!("service:{name}"),
                                json!({"name": name, "previous_status": prev_status, "status": status}),
                            ))
                            .await;
                    }
                    if prev_start_type != start_type {
                        let _ = sender
                            .send(EventInput::new(
                                "service",
                                "start_type_changed",
                                format!("service:{name}"),
                                json!({"name": name, "previous_start_type": prev_start_type, "start_type": start_type}),
                            ))
                            .await;
                    }
                }
            }
            previous = current_map;
        }
        tokio::time::sleep(config.services_interval).await;
    }
}

async fn run_software_monitor(sender: Sender<EventInput>, config: MonitorConfig) {
    let collector = SoftwareCollector;
    let mut previous: HashMap<String, (String, String)> = HashMap::new();
    let mut initialized = false;
    loop {
        if let Some(current) = collect_typed::<_, SoftwareInfo>(&collector).await {
            let mut current_map = HashMap::new();
            for app in &current.installed_programs {
                let key = software_identity_key(app);
                current_map.insert(key, (app.version.clone(), app.publisher.clone()));
            }

            if !initialized {
                previous = current_map;
                initialized = true;
                tokio::time::sleep(config.software_interval).await;
                continue;
            }

            for (identity_key, (version, publisher)) in &current_map {
                let Some(app) = current
                    .installed_programs
                    .iter()
                    .find(|a| software_identity_key(a) == *identity_key)
                else {
                    continue;
                };

                match previous.get(identity_key) {
                    None => {
                        let _ = sender
                            .send(EventInput::new(
                                "software",
                                "installed",
                                software_scope_key(app, identity_key),
                                json!({"name": app.name, "version": version, "publisher": publisher}),
                            ))
                            .await;
                    }
                    Some((prev_version, prev_publisher))
                        if prev_version != version || prev_publisher != publisher =>
                    {
                        let _ = sender
                            .send(EventInput::new(
                                "software",
                                "updated",
                                software_scope_key(app, identity_key),
                                json!({
                                    "name": app.name,
                                    "previous_version": prev_version,
                                    "version": version,
                                    "previous_publisher": prev_publisher,
                                    "publisher": publisher
                                }),
                            ))
                            .await;
                    }
                    _ => {}
                }
            }

            for (identity_key, (version, publisher)) in &previous {
                if !current_map.contains_key(identity_key) {
                    let inferred_name = identity_key
                        .split("::")
                        .next()
                        .unwrap_or(identity_key.as_str())
                        .to_string();
                    let _ = sender
                        .send(EventInput::new(
                            "software",
                            "uninstalled",
                            format!("software:{}:{}", inferred_name.to_lowercase(), short_identity(identity_key)),
                            json!({"name": inferred_name, "version": version, "publisher": publisher}),
                        ))
                        .await;
                }
            }
            previous = current_map;
        }
        tokio::time::sleep(config.software_interval).await;
    }
}

fn software_identity_key(app: &InstalledProgram) -> String {
    let name = app.name.trim().to_lowercase();
    let publisher = app.publisher.trim().to_lowercase();
    let uninstall = app
        .uninstall_string
        .clone()
        .unwrap_or_default()
        .trim()
        .to_lowercase();
    let location = app
        .location
        .clone()
        .unwrap_or_default()
        .trim()
        .to_lowercase();

    if let Some(product_code) = extract_msi_product_code(&uninstall) {
        return format!("{name}::{publisher}::msi:{product_code}");
    }
    if !uninstall.is_empty() {
        return format!("{name}::{publisher}::uninstall:{uninstall}");
    }
    if !location.is_empty() {
        return format!("{name}::{publisher}::location:{location}");
    }
    format!(
        "{name}::{publisher}::source:{}::x64:{}",
        app.source, app.is_64_bit
    )
}

fn software_scope_key(app: &InstalledProgram, identity_key: &str) -> String {
    format!(
        "software:{}:{}",
        app.name.to_lowercase(),
        short_identity(identity_key)
    )
}

fn short_identity(identity_key: &str) -> String {
    identity_key
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .take(12)
        .collect()
}

fn extract_msi_product_code(uninstall: &str) -> Option<String> {
    let start = uninstall.find('{')?;
    let end = uninstall[start..].find('}')?;
    let guid = &uninstall[start..start + end + 1];
    if guid.len() >= 10 {
        Some(guid.to_string())
    } else {
        None
    }
}

async fn run_network_monitor(sender: Sender<EventInput>, config: MonitorConfig) {
    let collector = NetworkCollector;
    let mut previous: HashMap<String, String> = HashMap::new();
    let mut pending_changes: HashMap<String, (String, u8)> = HashMap::new();
    let mut initialized = false;
    loop {
        if let Some(current) = collect_typed::<_, NetworkInfo>(&collector).await {
            let mut current_map = HashMap::new();
            for adapter in &current.adapters {
                let key = if adapter.mac_address.is_empty() {
                    adapter.name.clone()
                } else {
                    format!("{}:{}", adapter.name, adapter.mac_address)
                };
                let fingerprint = network_fingerprint(adapter);
                current_map.insert(key.clone(), fingerprint.clone());

                if !initialized {
                    continue;
                }

                match previous.get(&key) {
                    None => {
                        let _ = sender
                            .send(EventInput::new(
                                "network",
                                "adapter_added",
                                format!("network:{key}"),
                                json!({"adapter_key": key}),
                            ))
                            .await;
                    }
                    Some(prev) if prev != &fingerprint => {
                        let mut should_emit = false;
                        match pending_changes.get_mut(&key) {
                            Some((pending_fp, seen_count)) if pending_fp == &fingerprint => {
                                *seen_count = seen_count.saturating_add(1);
                                if *seen_count >= 2 {
                                    should_emit = true;
                                }
                            }
                            Some((pending_fp, seen_count)) => {
                                *pending_fp = fingerprint.clone();
                                *seen_count = 1;
                            }
                            None => {
                                pending_changes.insert(key.clone(), (fingerprint.clone(), 1));
                            }
                        }

                        if should_emit {
                            let _ = sender
                                .send(EventInput::new(
                                    "network",
                                    "identity_changed",
                                    format!("network:{key}"),
                                    json!({"adapter_key": key}),
                                ))
                                .await;
                            pending_changes.remove(&key);
                        }
                    }
                    _ => {
                        pending_changes.remove(&key);
                    }
                }
            }

            for key in previous.keys() {
                if initialized && !current_map.contains_key(key) {
                    let _ = sender
                        .send(EventInput::new(
                            "network",
                            "adapter_removed",
                            format!("network:{key}"),
                            json!({"adapter_key": key}),
                        ))
                        .await;
                }
                pending_changes.remove(key);
            }
            previous = current_map;
            initialized = true;
        }
        tokio::time::sleep(config.network_interval).await;
    }
}

fn network_fingerprint(adapter: &crate::models::NetworkAdapterConfig) -> String {
    let mut ips: Vec<String> = adapter
        .ips
        .iter()
        .map(|ip| format!("{}/{}:{}", ip.address, ip.prefix, ip.family))
        .collect();
    ips.sort();

    let mut gateways = adapter.gateways.clone();
    gateways.sort();

    let mut dns_servers = adapter.dns_servers.clone();
    dns_servers.sort();

    json!({
        "ips": ips,
        "gateways": gateways,
        "dns_servers": dns_servers,
        "dns_suffix": adapter.dns_suffix,
    })
    .to_string()
}

async fn run_system_monitor(
    sender: Sender<EventInput>,
    config: MonitorConfig,
    boot_session_id: String,
) {
    let collector = SystemCollector;
    if should_emit_boot_event(config.boot_event_max_start_delay_secs) {
        let _ = sender
            .send(EventInput::new(
                "system",
                "boot",
                format!("system:{boot_session_id}"),
                json!({"boot_session_id": boot_session_id}),
            ))
            .await;
    }

    let mut previous_hostname: Option<String> = None;
    loop {
        if let Some(current) = collect_typed::<_, SystemInfo>(&collector).await {
            match &previous_hostname {
                Some(prev) if prev != &current.hostname => {
                    let _ = sender
                        .send(EventInput::new(
                            "system",
                            "hostname_changed",
                            "system:hostname",
                            json!({"previous_hostname": prev, "hostname": current.hostname}),
                        ))
                        .await;
                }
                _ => {}
            }
            previous_hostname = Some(current.hostname);
        }
        tokio::time::sleep(config.system_interval).await;
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

async fn run_updates_monitor(sender: Sender<EventInput>, config: MonitorConfig) {
    let collector = UpdatesCollector;
    let mut previous_reboot_pending = None;
    let mut seen_history = 0usize;
    let mut initialized = false;
    loop {
        if let Some(current) = collect_typed::<_, UpdatesInfo>(&collector).await {
            let reboot_pending = current
                .windows_update
                .pending_updates
                .iter()
                .any(|u| u.requires_reboot);

            if !initialized {
                previous_reboot_pending = Some(reboot_pending);
                seen_history = current.update_history.len();
                initialized = true;
                tokio::time::sleep(config.updates_interval).await;
                continue;
            }

            if previous_reboot_pending.is_some() && previous_reboot_pending != Some(reboot_pending)
            {
                let _ = sender
                    .send(EventInput::new(
                        "updates",
                        "reboot_pending",
                        "updates:reboot",
                        json!({"reboot_pending": reboot_pending}),
                    ))
                    .await;
            }
            previous_reboot_pending = Some(reboot_pending);

            if current.update_history.len() > seen_history {
                for entry in current.update_history.iter().skip(seen_history) {
                    let result = entry.result.to_ascii_lowercase();
                    let event_kind = if result.contains("failed") {
                        "install_failed"
                    } else if result.contains("succeeded") {
                        "install_completed"
                    } else {
                        "install_started"
                    };
                    let _ = sender
                        .send(EventInput::new(
                            "updates",
                            event_kind,
                            format!("updates:{}", entry.title),
                            json!({"title": entry.title, "result": entry.result, "operation": entry.operation}),
                        ))
                        .await;
                }
                seen_history = current.update_history.len();
            }
        }
        tokio::time::sleep(config.updates_interval).await;
    }
}

async fn run_scheduled_tasks_monitor(sender: Sender<EventInput>, config: MonitorConfig) {
    let collector = ScheduledTasksCollector;
    let mut previous: HashMap<String, String> = HashMap::new();
    let mut initialized = false;
    loop {
        if let Some(current) = collect_typed::<_, ScheduledTasksInfo>(&collector).await {
            let mut current_map = HashMap::new();
            for task in &current.tasks {
                let key = format!("{}\\{}", task.path, task.name);
                current_map.insert(key.clone(), task.state.clone());

                if !initialized {
                    continue;
                }

                match previous.get(&key) {
                    None => {
                        let _ = sender
                            .send(EventInput::new(
                                "scheduled_task",
                                "created",
                                format!("scheduled_task:{key}"),
                                json!({"task": key}),
                            ))
                            .await;
                    }
                    Some(prev_state) if prev_state != &task.state => {
                        let _ = sender
                            .send(EventInput::new(
                                "scheduled_task",
                                "state_changed",
                                format!("scheduled_task:{key}"),
                                json!({"task": key, "previous_state": prev_state, "state": task.state}),
                            ))
                            .await;
                    }
                    _ => {}
                }
            }
            for key in previous.keys() {
                if initialized && !current_map.contains_key(key) {
                    let _ = sender
                        .send(EventInput::new(
                            "scheduled_task",
                            "deleted",
                            format!("scheduled_task:{key}"),
                            json!({"task": key}),
                        ))
                        .await;
                }
            }
            previous = current_map;
            initialized = true;
        }
        tokio::time::sleep(config.scheduled_tasks_interval).await;
    }
}

async fn run_sessions_monitor(sender: Sender<EventInput>, config: MonitorConfig) {
    let collector = SessionsCollector;
    let mut previous_ids: HashSet<String> = HashSet::new();
    let mut initialized = false;
    loop {
        if let Some(current) = collect_typed::<_, SessionsInfo>(&collector).await {
            let current_ids: HashSet<String> = current
                .sessions
                .iter()
                .map(|s| s.session_id.clone())
                .collect();

            if !initialized {
                previous_ids = current_ids;
                initialized = true;
                tokio::time::sleep(config.sessions_interval).await;
                continue;
            }

            for session in &current.sessions {
                if !previous_ids.contains(&session.session_id) {
                    let _ = sender
                        .send(EventInput::new(
                            "session",
                            "logon",
                            format!("session:{}", session.session_id),
                            json!({"session_id": session.session_id, "user": session.user, "domain": session.domain}),
                        ))
                        .await;
                }
            }

            for session_id in previous_ids.difference(&current_ids) {
                let _ = sender
                    .send(EventInput::new(
                        "session",
                        "logoff",
                        format!("session:{session_id}"),
                        json!({"session_id": session_id}),
                    ))
                    .await;
            }

            previous_ids = current_ids;
        }
        tokio::time::sleep(config.sessions_interval).await;
    }
}

async fn run_security_monitor(sender: Sender<EventInput>, config: MonitorConfig) {
    let collector = SecurityCollector;
    let mut previous_signature: Option<String> = None;
    let mut previous_realtime: Option<bool> = None;
    loop {
        if let Some(current) = collect_typed::<_, SecurityInfo>(&collector).await {
            let defender = current.antivirus.windows_defender;
            if let Some(sig) = defender.definition_version.clone() {
                if let Some(prev) = previous_signature.as_ref() {
                    if prev != &sig {
                        let _ = sender
                            .send(EventInput::new(
                                "security",
                                "defender_signature_updated",
                                "security:defender_signature",
                                json!({"previous_version": prev, "version": sig}),
                            ))
                            .await;
                    }
                }
                previous_signature = Some(sig);
            }

            if let Some(prev) = previous_realtime {
                if prev != defender.real_time_protection {
                    let _ = sender
                        .send(EventInput::new(
                            "security",
                            "defender_state_changed",
                            "security:defender_rtp",
                            json!({"previous_real_time_protection": prev, "real_time_protection": defender.real_time_protection}),
                        ))
                        .await;
                }
            }
            previous_realtime = Some(defender.real_time_protection);
        }
        tokio::time::sleep(config.security_interval).await;
    }
}

async fn run_disk_monitor(sender: Sender<EventInput>, config: MonitorConfig) {
    let mut active_alerts: BTreeSet<String> = BTreeSet::new();
    let mut initialized = false;
    loop {
        let mut disks = Disks::new_with_refreshed_list();
        disks.refresh();

        for disk in disks.list() {
            let name = disk.mount_point().to_string_lossy().to_string();
            let total = disk.total_space();
            if total == 0 {
                continue;
            }
            let available = disk.available_space();
            let free_percent = (available as f64 / total as f64) * 100.0;
            let in_low_space = free_percent < 10.0;

            if !initialized {
                if in_low_space {
                    active_alerts.insert(name.clone());
                }
                continue;
            }

            if in_low_space && !active_alerts.contains(&name) {
                let _ = sender
                    .send(EventInput::new(
                        "disk",
                        "low_space",
                        format!("disk:{name}"),
                        json!({"mount_point": name, "free_percent": free_percent, "available_bytes": available, "total_bytes": total}),
                    ))
                    .await;
                active_alerts.insert(name.clone());
            }
            if !in_low_space {
                active_alerts.remove(&name);
            }
        }

        initialized = true;
        tokio::time::sleep(config.disk_interval).await;
    }
}

async fn run_eventlog_monitor(sender: Sender<EventInput>, config: MonitorConfig) {
    let collector = EventsCollector;
    let mut previous_critical_24h = 0u32;
    loop {
        if let Some(current) = collect_typed::<_, EventsSummary>(&collector).await {
            let critical_24h = current.summary.critical_last_24h;
            if previous_critical_24h > 0 && critical_24h > previous_critical_24h {
                let _ = sender
                    .send(EventInput::new(
                        "eventlog",
                        "critical",
                        "eventlog:critical",
                        json!({
                            "previous_critical_last_24h": previous_critical_24h,
                            "critical_last_24h": critical_24h
                        }),
                    ))
                    .await;
            }
            previous_critical_24h = critical_24h;
        }
        tokio::time::sleep(config.eventlog_interval).await;
    }
}

async fn run_certificate_monitor(sender: Sender<EventInput>, config: MonitorConfig) {
    let collector = CertificatesCollector;
    let mut seen_alerts: HashSet<String> = HashSet::new();
    let mut initialized = false;
    loop {
        if let Some(current) = collect_typed::<_, CertificatesInfo>(&collector).await {
            let cutoff = chrono::Utc::now() + chrono::Duration::days(config.cert_expiring_days);
            for cert in current.stores.iter().flat_map(|s| &s.certificates) {
                let Some(expires_at) = cert.not_after else {
                    continue;
                };
                if expires_at > cutoff {
                    continue;
                }

                if !initialized {
                    seen_alerts.insert(cert.thumbprint.clone());
                    continue;
                }

                if seen_alerts.insert(cert.thumbprint.clone()) {
                    let _ = sender
                        .send(EventInput::new(
                            "certificate",
                            "expiring_soon",
                            format!("certificate:{}", cert.thumbprint),
                            json!({
                                "thumbprint": cert.thumbprint,
                                "subject": cert.subject,
                                "issuer": cert.issuer,
                                "not_after": expires_at,
                            }),
                        ))
                        .await;
                }
            }
            initialized = true;
        }
        tokio::time::sleep(config.certificates_interval).await;
    }
}

async fn collect_typed<C, T>(collector: &C) -> Option<T>
where
    C: Collector,
    T: DeserializeOwned,
{
    match collector.collect().await {
        Ok(value) => serde_json::from_value(value).ok(),
        Err(error) => {
            warn!(collector = collector.name(), %error, "event monitor collection failed");
            None
        }
    }
}
