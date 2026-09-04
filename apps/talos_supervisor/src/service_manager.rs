use std::{
    path::{Path, PathBuf},
    process::{Command, Output},
    thread,
    time::{Duration, Instant},
};

#[cfg(any(target_os = "linux", target_os = "macos"))]
use std::fs;

use anyhow::{bail, Context, Result};
use tracing::{debug, info, warn};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ServiceState {
    Running,
    Stopped,
    Starting,
    Stopping,
    Unknown,
}

impl ServiceState {
    fn matches(self, expected: ServiceState) -> bool {
        self == expected
    }
}

pub(crate) struct WorkerServiceConfig {
    pub(crate) service_name: String,
    pub(crate) display_name: String,
    pub(crate) description: String,
    pub(crate) executable_path: PathBuf,
    pub(crate) environment_file: Option<PathBuf>,
}

pub(crate) trait ServiceManager {
    fn service_exists(&self, service_name: &str) -> Result<bool>;
    fn service_state(&self, service_name: &str) -> Result<Option<ServiceState>>;
    fn install_worker_service(&self, config: &WorkerServiceConfig) -> Result<()>;
    fn ensure_worker_service(&self, config: &WorkerServiceConfig) -> Result<bool> {
        if self.service_exists(&config.service_name)? {
            return Ok(false);
        }
        self.install_worker_service(config)?;
        Ok(true)
    }
    fn start_service(&self, service_name: &str) -> Result<()>;
    fn stop_service(&self, service_name: &str) -> Result<()>;
    fn ensure_worker_firewall_rule(&self, _worker_exe: &Path) {}

    fn wait_for_service_state(
        &self,
        service_name: &str,
        expected: ServiceState,
        timeout: Duration,
    ) -> Result<()> {
        let deadline = Instant::now() + timeout;
        loop {
            match self.service_state(service_name)? {
                Some(state) if state.matches(expected) => {
                    debug!(
                        service_name,
                        ?expected,
                        ?state,
                        "service reached expected state"
                    );
                    return Ok(());
                }
                None if expected == ServiceState::Stopped => {
                    debug!(service_name, "service is absent; treating as stopped");
                    return Ok(());
                }
                Some(state) => {
                    if Instant::now() >= deadline {
                        bail!(
                            "timed out waiting for service '{}' to reach {:?} (current: {:?})",
                            service_name,
                            expected,
                            state
                        );
                    }
                }
                None => {
                    if Instant::now() >= deadline {
                        bail!(
                            "timed out waiting for service '{}' to reach {:?} (service missing)",
                            service_name,
                            expected
                        );
                    }
                }
            }
            thread::sleep(Duration::from_secs(1));
        }
    }
}

pub(crate) fn platform_service_manager() -> Result<Box<dyn ServiceManager>> {
    #[cfg(target_os = "windows")]
    {
        return Ok(Box::new(WindowsServiceManager));
    }
    #[cfg(target_os = "linux")]
    {
        if SystemdServiceManager::is_available() {
            return Ok(Box::new(SystemdServiceManager));
        }
        bail!("unsupported_service_manager: systemd is not available");
    }
    #[cfg(target_os = "macos")]
    {
        return Ok(Box::new(LaunchdServiceManager));
    }
    #[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
    {
        bail!("unsupported_service_manager: this platform is not supported by Talos Supervisor");
    }
}

#[cfg(target_os = "windows")]
struct WindowsServiceManager;

#[cfg(target_os = "windows")]
impl WindowsServiceManager {
    const WORKER_FIREWALL_RULE_NAME: &'static str = "Talos Worker QUIC UDP Inbound";

    fn sc_output(args: &[&str]) -> Result<Output> {
        Command::new("sc.exe")
            .args(args)
            .output()
            .with_context(|| format!("run sc.exe {}", args.join(" ")))
    }

    fn sc_status(args: &[&str]) -> Result<std::process::ExitStatus> {
        Command::new("sc.exe")
            .args(args)
            .status()
            .with_context(|| format!("run sc.exe {}", args.join(" ")))
    }

    fn worker_firewall_rule_exists() -> Result<bool> {
        let name_arg = format!("name={}", Self::WORKER_FIREWALL_RULE_NAME);
        let status = Command::new("netsh.exe")
            .args(["advfirewall", "firewall", "show", "rule", name_arg.as_str()])
            .status()
            .with_context(|| {
                format!(
                    "run netsh.exe advfirewall firewall show rule name={}",
                    Self::WORKER_FIREWALL_RULE_NAME
                )
            })?;
        Ok(status.success())
    }
}

#[cfg(target_os = "windows")]
impl ServiceManager for WindowsServiceManager {
    fn service_exists(&self, service_name: &str) -> Result<bool> {
        Ok(self.service_state(service_name)?.is_some())
    }

    fn service_state(&self, service_name: &str) -> Result<Option<ServiceState>> {
        let output = Self::sc_output(&["query", service_name])?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let combined = format!("{stdout}\n{stderr}");
        if !output.status.success()
            && (combined.contains("1060") || combined.contains("does not exist"))
        {
            return Ok(None);
        }
        if !output.status.success() {
            return Ok(None);
        }
        for line in stdout.lines() {
            let trimmed = line.trim();
            if !trimmed.starts_with("STATE") {
                continue;
            }
            if trimmed.contains("RUNNING") {
                return Ok(Some(ServiceState::Running));
            }
            if trimmed.contains("STOPPED") {
                return Ok(Some(ServiceState::Stopped));
            }
            if trimmed.contains("START_PENDING") {
                return Ok(Some(ServiceState::Starting));
            }
            if trimmed.contains("STOP_PENDING") {
                return Ok(Some(ServiceState::Stopping));
            }
            return Ok(Some(ServiceState::Unknown));
        }
        Ok(Some(ServiceState::Unknown))
    }

    fn install_worker_service(&self, config: &WorkerServiceConfig) -> Result<()> {
        let _ = &config.environment_file;
        let worker_exe = format!("\"{}\"", config.executable_path.display());
        let status = Self::sc_status(&[
            "create",
            &config.service_name,
            "binPath=",
            &worker_exe,
            "start=",
            "auto",
            "DisplayName=",
            &config.display_name,
        ])
        .context("create Talos Worker service")?;
        if !status.success() {
            bail!(
                "sc create {} failed with code {:?}",
                config.service_name,
                status.code()
            );
        }
        let _ = Self::sc_status(&["description", &config.service_name, &config.description]);
        info!(
            service_name = %config.service_name,
            worker_exe = %config.executable_path.display(),
            "created Talos Worker service"
        );
        Ok(())
    }

    fn start_service(&self, service_name: &str) -> Result<()> {
        debug!(service_name, "starting service");
        let status = Self::sc_status(&["start", service_name]).context("start service")?;
        if !status.success() {
            warn!(service_name, code = ?status.code(), "sc start returned non-success; waiting for running state anyway");
        }
        self.wait_for_service_state(service_name, ServiceState::Running, Duration::from_secs(60))
    }

    fn stop_service(&self, service_name: &str) -> Result<()> {
        debug!(service_name, "stopping service");
        match Self::sc_status(&["stop", service_name]) {
            Ok(status) if status.success() => {
                debug!(service_name, code = ?status.code(), "sc stop issued");
            }
            Ok(status) => {
                warn!(service_name, code = ?status.code(), "sc stop returned non-success")
            }
            Err(err) => warn!(service_name, error = %err, "sc stop failed to run"),
        }
        self.wait_for_service_state(service_name, ServiceState::Stopped, Duration::from_secs(60))
    }

    fn ensure_worker_firewall_rule(&self, worker_exe: &Path) {
        match Self::worker_firewall_rule_exists() {
            Ok(true) => {
                info!(
                    rule = Self::WORKER_FIREWALL_RULE_NAME,
                    "Talos Worker firewall rule already present"
                );
                return;
            }
            Ok(false) => {}
            Err(err) => warn!(
                rule = Self::WORKER_FIREWALL_RULE_NAME,
                error = %err,
                "failed to check Talos Worker firewall rule before create"
            ),
        }

        let program = worker_exe.to_string_lossy().to_string();
        let program_arg = format!("program={program}");
        let name_arg = format!("name={}", Self::WORKER_FIREWALL_RULE_NAME);
        let status = Command::new("netsh.exe")
            .args([
                "advfirewall",
                "firewall",
                "add",
                "rule",
                name_arg.as_str(),
                "dir=in",
                "action=allow",
                program_arg.as_str(),
                "protocol=UDP",
                "profile=any",
                "remoteip=localsubnet",
            ])
            .status();
        match status {
            Ok(status) if status.success() => {
                info!(worker_exe = %worker_exe.display(), "ensured Talos Worker firewall rule");
            }
            Ok(status) => warn!(
                worker_exe = %worker_exe.display(),
                code = ?status.code(),
                "netsh returned non-success while creating Talos Worker firewall rule"
            ),
            Err(err) => warn!(
                worker_exe = %worker_exe.display(),
                error = %err,
                "failed to run netsh for Talos Worker firewall rule"
            ),
        }
    }
}

#[cfg(target_os = "linux")]
struct SystemdServiceManager;

#[cfg(target_os = "linux")]
impl SystemdServiceManager {
    fn is_available() -> bool {
        Path::new("/run/systemd/system").exists() && command_available("systemctl")
    }

    fn unit_name(service_name: &str) -> Result<String> {
        let trimmed = service_name.trim();
        if trimmed.is_empty() {
            bail!("service name is empty");
        }
        if trimmed.contains('/') || trimmed.contains('\\') {
            bail!("service name must not contain path separators: {trimmed}");
        }
        if trimmed.ends_with(".service") {
            Ok(trimmed.to_string())
        } else {
            Ok(format!("{trimmed}.service"))
        }
    }

    fn unit_path(service_name: &str) -> Result<PathBuf> {
        Ok(PathBuf::from("/etc/systemd/system").join(Self::unit_name(service_name)?))
    }

    fn systemctl(args: &[&str]) -> Result<Output> {
        Command::new("systemctl")
            .args(args)
            .output()
            .with_context(|| format!("run systemctl {}", args.join(" ")))
    }

    fn systemctl_status(args: &[&str]) -> Result<std::process::ExitStatus> {
        Command::new("systemctl")
            .args(args)
            .status()
            .with_context(|| format!("run systemctl {}", args.join(" ")))
    }

    fn daemon_reload() -> Result<()> {
        let status = Self::systemctl_status(&["daemon-reload"])?;
        if !status.success() {
            bail!(
                "systemctl daemon-reload failed with code {:?}",
                status.code()
            );
        }
        Ok(())
    }

    fn enable_service(service_name: &str) -> Result<()> {
        let unit = Self::unit_name(service_name)?;
        let status = Self::systemctl_status(&["enable", &unit])?;
        if !status.success() {
            bail!(
                "systemctl enable {unit} failed with code {:?}",
                status.code()
            );
        }
        Ok(())
    }
}

#[cfg(target_os = "linux")]
impl ServiceManager for SystemdServiceManager {
    fn service_exists(&self, service_name: &str) -> Result<bool> {
        let unit = Self::unit_name(service_name)?;
        let output = Self::systemctl(&["show", &unit, "--property=LoadState", "--value"])?;
        let load_state = String::from_utf8_lossy(&output.stdout).trim().to_string();
        Ok(output.status.success() && !load_state.is_empty() && load_state != "not-found")
    }

    fn service_state(&self, service_name: &str) -> Result<Option<ServiceState>> {
        if !self.service_exists(service_name)? {
            return Ok(None);
        }
        let unit = Self::unit_name(service_name)?;
        let output = Self::systemctl(&["is-active", &unit])?;
        let active_state = String::from_utf8_lossy(&output.stdout).trim().to_string();
        match active_state.as_str() {
            "active" => Ok(Some(ServiceState::Running)),
            "activating" => Ok(Some(ServiceState::Starting)),
            "deactivating" => Ok(Some(ServiceState::Stopping)),
            "inactive" => Ok(Some(ServiceState::Stopped)),
            "failed" => Ok(Some(ServiceState::Stopped)),
            _ => Ok(Some(ServiceState::Unknown)),
        }
    }

    fn install_worker_service(&self, config: &WorkerServiceConfig) -> Result<()> {
        let unit_path = Self::unit_path(&config.service_name)?;
        if let Some(parent) = unit_path.parent() {
            fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
        }
        let unit = worker_systemd_unit(config)?;
        fs::write(&unit_path, unit).with_context(|| format!("write {}", unit_path.display()))?;
        Self::daemon_reload()?;
        Self::enable_service(&config.service_name)?;
        info!(
            service_name = %config.service_name,
            unit_path = %unit_path.display(),
            worker_exe = %config.executable_path.display(),
            "created Talos Worker systemd unit"
        );
        Ok(())
    }

    fn ensure_worker_service(&self, config: &WorkerServiceConfig) -> Result<bool> {
        let unit_path = Self::unit_path(&config.service_name)?;
        if let Some(parent) = unit_path.parent() {
            fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
        }
        let unit = worker_systemd_unit(config)?;
        let current = fs::read_to_string(&unit_path).ok();
        let changed = current.as_deref() != Some(unit.as_str());
        if changed {
            fs::write(&unit_path, unit)
                .with_context(|| format!("write {}", unit_path.display()))?;
            Self::daemon_reload()?;
            info!(
                service_name = %config.service_name,
                unit_path = %unit_path.display(),
                worker_exe = %config.executable_path.display(),
                "updated Talos Worker systemd unit"
            );
        } else {
            debug!(
                service_name = %config.service_name,
                unit_path = %unit_path.display(),
                "Talos Worker systemd unit already matches desired configuration"
            );
        }
        Self::enable_service(&config.service_name)?;
        Ok(changed)
    }

    fn start_service(&self, service_name: &str) -> Result<()> {
        let unit = Self::unit_name(service_name)?;
        debug!(service_name = %unit, "starting systemd service");
        let status = Self::systemctl_status(&["start", &unit])?;
        if !status.success() {
            bail!(
                "systemctl start {unit} failed with code {:?}",
                status.code()
            );
        }
        self.wait_for_service_state(service_name, ServiceState::Running, Duration::from_secs(60))
    }

    fn stop_service(&self, service_name: &str) -> Result<()> {
        let unit = Self::unit_name(service_name)?;
        debug!(service_name = %unit, "stopping systemd service");
        let status = Self::systemctl_status(&["stop", &unit])?;
        if !status.success() {
            warn!(service_name = %unit, code = ?status.code(), "systemctl stop returned non-success");
        }
        self.wait_for_service_state(service_name, ServiceState::Stopped, Duration::from_secs(60))
    }
}

#[cfg(target_os = "macos")]
struct LaunchdServiceManager;

#[cfg(target_os = "macos")]
impl LaunchdServiceManager {
    fn validate_label(service_name: &str) -> Result<String> {
        let label = service_name.trim();
        if label.is_empty() {
            bail!("launchd service label is empty");
        }
        if label.contains('/') || label.contains('\\') {
            bail!("launchd service label must not contain path separators: {label}");
        }
        Ok(label.to_string())
    }

    fn service_target(service_name: &str) -> Result<String> {
        Ok(format!("system/{}", Self::validate_label(service_name)?))
    }

    fn plist_path(service_name: &str) -> Result<PathBuf> {
        Ok(PathBuf::from("/Library/LaunchDaemons")
            .join(format!("{}.plist", Self::validate_label(service_name)?)))
    }

    fn launchctl(args: &[&str]) -> Result<Output> {
        Command::new("launchctl")
            .args(args)
            .output()
            .with_context(|| format!("run launchctl {}", args.join(" ")))
    }

    fn launchctl_status(args: &[&str]) -> Result<std::process::ExitStatus> {
        Command::new("launchctl")
            .args(args)
            .status()
            .with_context(|| format!("run launchctl {}", args.join(" ")))
    }

    fn is_loaded(service_name: &str) -> Result<bool> {
        let target = Self::service_target(service_name)?;
        Ok(Self::launchctl(&["print", &target])
            .map(|output| output.status.success())
            .unwrap_or(false))
    }
}

#[cfg(target_os = "macos")]
impl ServiceManager for LaunchdServiceManager {
    fn service_exists(&self, service_name: &str) -> Result<bool> {
        Ok(Self::is_loaded(service_name)? || Self::plist_path(service_name)?.exists())
    }

    fn service_state(&self, service_name: &str) -> Result<Option<ServiceState>> {
        let target = Self::service_target(service_name)?;
        let output = Self::launchctl(&["print", &target])?;
        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            for line in stdout.lines() {
                let trimmed = line.trim();
                if trimmed.starts_with("pid = ") {
                    return Ok(Some(ServiceState::Running));
                }
                if let Some((_, state)) = trimmed.split_once("state = ") {
                    let state = state.trim();
                    if state.eq_ignore_ascii_case("spawning") {
                        return Ok(Some(ServiceState::Starting));
                    }
                    if state.eq_ignore_ascii_case("removing")
                        || state.eq_ignore_ascii_case("exiting")
                    {
                        return Ok(Some(ServiceState::Stopping));
                    }
                    if state.eq_ignore_ascii_case("running") {
                        return Ok(Some(ServiceState::Running));
                    }
                    if state.eq_ignore_ascii_case("waiting") || state.eq_ignore_ascii_case("exited")
                    {
                        return Ok(Some(ServiceState::Stopped));
                    }
                    return Ok(Some(ServiceState::Unknown));
                }
            }
            return Ok(Some(ServiceState::Stopped));
        }
        if Self::plist_path(service_name)?.exists() {
            return Ok(Some(ServiceState::Stopped));
        }
        Ok(None)
    }

    fn install_worker_service(&self, config: &WorkerServiceConfig) -> Result<()> {
        let label = Self::validate_label(&config.service_name)?;
        let plist_path = Self::plist_path(&label)?;
        if let Some(parent) = plist_path.parent() {
            fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
        }
        if let Some(parent) = config.executable_path.parent() {
            fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
        }
        fs::create_dir_all("/Library/Logs/Talos").context("create /Library/Logs/Talos")?;

        remove_legacy_worker_launchd_wrapper(&config.executable_path)?;
        let environment = worker_launchd_environment(config.environment_file.as_deref())?;
        let plist = worker_launchd_plist(&label, &config.executable_path, &environment);
        let target = Self::service_target(&label)?;
        let _ = Self::launchctl_status(&["bootout", &target]);
        fs::write(&plist_path, plist).with_context(|| format!("write {}", plist_path.display()))?;
        info!(
            service_name = %label,
            display_name = %config.display_name,
            description = %config.description,
            plist_path = %plist_path.display(),
            worker_exe = %config.executable_path.display(),
            "created Talos Worker launchd service"
        );
        Ok(())
    }

    fn ensure_worker_service(&self, config: &WorkerServiceConfig) -> Result<bool> {
        let label = Self::validate_label(&config.service_name)?;
        if !self.service_exists(&label)? {
            self.install_worker_service(config)?;
            return Ok(true);
        }

        let plist_path = Self::plist_path(&label)?;
        remove_legacy_worker_launchd_wrapper(&config.executable_path)?;
        let environment = worker_launchd_environment(config.environment_file.as_deref())?;
        let desired_plist = worker_launchd_plist(&label, &config.executable_path, &environment);
        let mut changed = false;

        if fs::read_to_string(&plist_path).ok().as_deref() != Some(desired_plist.as_str()) {
            if let Some(parent) = plist_path.parent() {
                fs::create_dir_all(parent)
                    .with_context(|| format!("create {}", parent.display()))?;
            }
            let target = Self::service_target(&label)?;
            let _ = Self::launchctl_status(&["bootout", &target]);
            fs::write(&plist_path, desired_plist)
                .with_context(|| format!("write {}", plist_path.display()))?;
            changed = true;
        }

        Ok(changed)
    }

    fn start_service(&self, service_name: &str) -> Result<()> {
        let label = Self::validate_label(service_name)?;
        let target = Self::service_target(&label)?;
        let plist_path = Self::plist_path(&label)?;
        if !Self::is_loaded(&label)? {
            let status = Self::launchctl_status(&[
                "bootstrap",
                "system",
                plist_path.to_string_lossy().as_ref(),
            ])?;
            if !status.success() {
                bail!(
                    "launchctl bootstrap system {} failed with code {:?}",
                    plist_path.display(),
                    status.code()
                );
            }
        }
        let status = Self::launchctl_status(&["enable", &target])?;
        if !status.success() {
            warn!(service_name = %label, code = ?status.code(), "launchctl enable returned non-success");
        }
        let status = Self::launchctl_status(&["kickstart", "-k", &target])?;
        if !status.success() {
            bail!(
                "launchctl kickstart {target} failed with code {:?}",
                status.code()
            );
        }
        self.wait_for_service_state(&label, ServiceState::Running, Duration::from_secs(60))
    }

    fn stop_service(&self, service_name: &str) -> Result<()> {
        let label = Self::validate_label(service_name)?;
        let target = Self::service_target(&label)?;
        match Self::launchctl_status(&["bootout", &target]) {
            Ok(status) if status.success() => {
                debug!(service_name = %label, code = ?status.code(), "launchctl bootout issued");
            }
            Ok(status) => {
                warn!(service_name = %label, code = ?status.code(), "launchctl bootout returned non-success")
            }
            Err(err) => {
                warn!(service_name = %label, error = %err, "launchctl bootout failed to run")
            }
        }
        self.wait_for_service_state(&label, ServiceState::Stopped, Duration::from_secs(60))
    }
}

#[cfg(target_os = "macos")]
fn legacy_worker_launchd_wrapper_path(worker_exe: &Path) -> PathBuf {
    let Some(macos_dir) = worker_exe.parent() else {
        return PathBuf::from("/Library/Talos/Worker/run-talos-worker.sh");
    };
    if macos_dir.file_name().and_then(|value| value.to_str()) == Some("MacOS") {
        if let Some(contents_dir) = macos_dir.parent() {
            if contents_dir.file_name().and_then(|value| value.to_str()) == Some("Contents") {
                if let Some(app_dir) = contents_dir.parent() {
                    if app_dir
                        .extension()
                        .and_then(|value| value.to_str())
                        .is_some_and(|value| value.eq_ignore_ascii_case("app"))
                    {
                        if let Some(install_dir) = app_dir.parent() {
                            return install_dir.join("run-talos-worker.sh");
                        }
                    }
                }
            }
        }
    }
    macos_dir.join("run-talos-worker.sh")
}

#[cfg(target_os = "macos")]
fn remove_legacy_worker_launchd_wrapper(worker_exe: &Path) -> Result<()> {
    let wrapper_path = legacy_worker_launchd_wrapper_path(worker_exe);
    if wrapper_path.exists() {
        fs::remove_file(&wrapper_path).with_context(|| {
            format!(
                "remove legacy worker launch wrapper {}",
                wrapper_path.display()
            )
        })?;
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn worker_launchd_plist(
    label: &str,
    worker_exe: &Path,
    environment: &[(String, String)],
) -> String {
    let working_directory = worker_launchd_working_directory(worker_exe);
    let environment_xml = worker_launchd_environment_xml(environment);
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\"\n\
  \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
<plist version=\"1.0\">\n\
<dict>\n\
  <key>Label</key>\n\
  <string>{}</string>\n\
  <key>ProgramArguments</key>\n\
  <array>\n\
    <string>{}</string>\n\
  </array>\n\
{}\
  <key>RunAtLoad</key>\n\
  <true/>\n\
  <key>KeepAlive</key>\n\
  <true/>\n\
  <key>WorkingDirectory</key>\n\
  <string>{}</string>\n\
  <key>StandardOutPath</key>\n\
  <string>/Library/Logs/Talos/talos_worker.launchd.out.log</string>\n\
  <key>StandardErrorPath</key>\n\
  <string>/Library/Logs/Talos/talos_worker.launchd.err.log</string>\n\
</dict>\n\
</plist>\n",
        xml_escape(label),
        xml_escape(&worker_exe.display().to_string()),
        environment_xml,
        xml_escape(&working_directory.display().to_string())
    )
}

#[cfg(target_os = "macos")]
fn worker_launchd_working_directory(worker_exe: &Path) -> PathBuf {
    let Some(macos_dir) = worker_exe.parent() else {
        return PathBuf::from("/Library/Talos/Worker");
    };
    if macos_dir.file_name().and_then(|value| value.to_str()) == Some("MacOS") {
        if let Some(contents_dir) = macos_dir.parent() {
            if contents_dir.file_name().and_then(|value| value.to_str()) == Some("Contents") {
                if let Some(app_dir) = contents_dir.parent() {
                    if app_dir
                        .extension()
                        .and_then(|value| value.to_str())
                        .is_some_and(|value| value.eq_ignore_ascii_case("app"))
                    {
                        if let Some(install_dir) = app_dir.parent() {
                            return install_dir.to_path_buf();
                        }
                    }
                }
            }
        }
    }
    macos_dir.to_path_buf()
}

#[cfg(target_os = "macos")]
fn worker_launchd_environment(path: Option<&Path>) -> Result<Vec<(String, String)>> {
    let Some(path) = path else {
        return Ok(Vec::new());
    };
    if !path.exists() {
        return Ok(Vec::new());
    }
    let contents = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    let mut environment = Vec::new();
    for (index, raw_line) in contents.lines().enumerate() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line = line.strip_prefix("export ").unwrap_or(line).trim_start();
        let Some((raw_key, raw_value)) = line.split_once('=') else {
            warn!(
                env_file = %path.display(),
                line = index + 1,
                "ignoring malformed worker environment line"
            );
            continue;
        };
        let key = raw_key.trim();
        if !is_valid_environment_key(key) {
            warn!(
                env_file = %path.display(),
                line = index + 1,
                key,
                "ignoring invalid worker environment key"
            );
            continue;
        }
        environment.push((key.to_string(), parse_environment_value(raw_value.trim())));
    }
    Ok(environment)
}

#[cfg(target_os = "macos")]
fn is_valid_environment_key(key: &str) -> bool {
    let mut chars = key.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first == '_' || first.is_ascii_alphabetic()) {
        return false;
    }
    chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
}

#[cfg(target_os = "macos")]
fn parse_environment_value(value: &str) -> String {
    let value = strip_unquoted_environment_comment(value).trim();
    if value.len() >= 2 {
        let bytes = value.as_bytes();
        if (bytes[0] == b'\'' && bytes[value.len() - 1] == b'\'')
            || (bytes[0] == b'"' && bytes[value.len() - 1] == b'"')
        {
            return value[1..value.len() - 1].to_string();
        }
    }
    value.to_string()
}

#[cfg(target_os = "macos")]
fn strip_unquoted_environment_comment(value: &str) -> &str {
    let mut quote: Option<char> = None;
    let mut previous_was_whitespace = false;
    for (index, ch) in value.char_indices() {
        match ch {
            '\'' | '"' => {
                if quote == Some(ch) {
                    quote = None;
                } else if quote.is_none() {
                    quote = Some(ch);
                }
            }
            '#' if quote.is_none() && (index == 0 || previous_was_whitespace) => {
                return &value[..index];
            }
            _ => {}
        }
        previous_was_whitespace = ch.is_whitespace();
    }
    value
}

#[cfg(target_os = "macos")]
fn worker_launchd_environment_xml(environment: &[(String, String)]) -> String {
    if environment.is_empty() {
        return String::new();
    }
    let mut xml = String::from("  <key>EnvironmentVariables</key>\n  <dict>\n");
    for (key, value) in environment {
        xml.push_str("    <key>");
        xml.push_str(&xml_escape(key));
        xml.push_str("</key>\n    <string>");
        xml.push_str(&xml_escape(value));
        xml.push_str("</string>\n");
    }
    xml.push_str("  </dict>\n");
    xml
}

#[cfg(target_os = "macos")]
fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(target_os = "linux")]
fn worker_systemd_unit(config: &WorkerServiceConfig) -> Result<String> {
    let exec_start = systemd_quote_path(&config.executable_path);
    let description = if config.description.trim().is_empty() {
        config.display_name.clone()
    } else {
        format!("{} ({})", config.display_name, config.description)
    };
    let environment_file = config
        .environment_file
        .as_ref()
        .map(|path| format!("EnvironmentFile={}\n", systemd_quote_path(path)))
        .unwrap_or_default();
    let read_write_paths = worker_read_write_paths(&config.executable_path);
    Ok(format!(
        "[Unit]\n\
Description={}\n\
After=network-online.target\n\
Wants=network-online.target\n\
\n\
[Service]\n\
Type=simple\n\
{}\
ExecStart={}\n\
Restart=always\n\
RestartSec=10\n\
User=root\n\
Group=root\n\
NoNewPrivileges=false\n\
PrivateTmp=true\n\
ProtectHome=read-only\n\
ReadWritePaths={}\n\
\n\
[Install]\n\
WantedBy=multi-user.target\n",
        systemd_escape_text(&description),
        environment_file,
        exec_start,
        read_write_paths
    ))
}

#[cfg(target_os = "linux")]
fn worker_read_write_paths(worker_exe: &Path) -> String {
    let worker_dir = worker_exe
        .parent()
        .unwrap_or_else(|| Path::new("/opt/talos/worker"));
    [
        Path::new("/etc/talos"),
        Path::new("/var/lib/talos"),
        Path::new("/var/log/talos"),
        Path::new("/tmp"),
        worker_dir,
    ]
    .iter()
    .map(|path| systemd_quote_path(path))
    .collect::<Vec<_>>()
    .join(" ")
}

#[cfg(target_os = "linux")]
fn systemd_quote_path(path: &Path) -> String {
    let value = path.display().to_string();
    if value.chars().any(char::is_whitespace) {
        format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
    } else {
        value
    }
}

#[cfg(target_os = "linux")]
fn systemd_escape_text(value: &str) -> String {
    value.replace('\n', " ").replace('\r', " ")
}

#[cfg(target_os = "linux")]
fn command_available(name: &str) -> bool {
    if Path::new(name).is_absolute() {
        return Path::new(name).is_file();
    }
    std::env::var_os("PATH")
        .map(|paths| std::env::split_paths(&paths).any(|dir| dir.join(name).is_file()))
        .unwrap_or(false)
}
