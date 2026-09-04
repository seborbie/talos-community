use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{anyhow, bail, Context, Result};
use reqwest::Client;
use talos_update_common::{
    download_file, fetch_manifest, is_update_newer, normalize_update_base_url, sha256_hex_bytes,
    sha256_hex_file, validate_manifest_context, verify_manifest_signature, verify_package_sha256,
    verify_package_size, ManifestFetchResult, UpdateManifestExpectation,
};
use tracing::{debug, info, warn};

use crate::{
    apply_update_package, promote_pending_supervisor,
    service_manager::{platform_service_manager, ServiceState, WorkerServiceConfig},
    ApplyArgs,
};

const EMBEDDED_MANIFEST_PUBLIC_KEY_DER: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/manifest_public_key.der"));
const UPDATE_HTTP_CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const UPDATE_HTTP_TIMEOUT: Duration = Duration::from_secs(90);
const UPDATE_REQUEST_ATTEMPTS: usize = 4;
#[cfg(target_os = "windows")]
const SUPERVISOR_SERVICE_NAME: &str = "TalosSupervisor";
#[cfg(target_os = "linux")]
const SUPERVISOR_SERVICE_NAME: &str = "talos-supervisor";
#[cfg(target_os = "macos")]
const SUPERVISOR_SERVICE_NAME: &str = "com.talos.talos-supervisor";
#[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
const SUPERVISOR_SERVICE_NAME: &str = "talos-supervisor";
#[cfg(target_os = "windows")]
const WORKER_SERVICE_NAME: &str = "TalosWorker";
#[cfg(target_os = "linux")]
const WORKER_SERVICE_NAME: &str = "talos-worker";
#[cfg(target_os = "macos")]
const WORKER_SERVICE_NAME: &str = "com.talos.talos-worker";
#[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
const WORKER_SERVICE_NAME: &str = "talos-worker";
pub(crate) const WORKER_SERVICE_NAME_FOR_APPLY: &str = WORKER_SERVICE_NAME;
const SUPERVISOR_PRODUCT: &str = "supervisor";
const WORKER_PRODUCT: &str = "worker";
const WORKER_VERSION_FILE_NAME: &str = "worker.version";
#[cfg(target_os = "windows")]
const WORKER_EXE_NAME: &str = "talos_worker.exe";
#[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
const WORKER_EXE_NAME: &str = "talos_worker";
#[cfg(target_os = "macos")]
const MACOS_WORKER_APP_EXE_RELATIVE_PATH: &str = "Talos Worker.app/Contents/MacOS/talos_worker";
#[cfg(target_os = "macos")]
const MACOS_WORKER_RESTART_REQUEST_PATH: &str = "/tmp/talos-worker-restart-request";
#[cfg(target_os = "windows")]
const LEGACY_WORKER_EXE_NAME: &str = "talos_worker.exe";
#[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
const LEGACY_WORKER_EXE_NAME: &str = "talos-rmm-agent";

pub(crate) struct SupervisorArgs {
    once: bool,
    update_base_url: Option<String>,
    channel: Option<String>,
    ring: Option<String>,
    worker_install_dir: Option<PathBuf>,
    worker_service_name: Option<String>,
    supervisor_service_name: Option<String>,
    startup_jitter_secs: Option<u64>,
    update_interval_secs: Option<u64>,
    monitor_interval_secs: Option<u64>,
}

struct SupervisorConfig {
    client: Client,
    update_base_url: Option<String>,
    channel: String,
    ring: Option<String>,
    worker_arch: String,
    worker_install_dir: PathBuf,
    worker_environment_file: Option<PathBuf>,
    worker_version_path: PathBuf,
    worker_service_name: String,
    supervisor_service_name: String,
    supervisor_install_dir: PathBuf,
    startup_jitter_secs: u64,
    update_interval: Duration,
    monitor_interval: Duration,
    once: bool,
}

impl SupervisorArgs {
    pub(crate) fn parse(raw_args: &[String]) -> Result<Self> {
        let mut args = Self {
            once: false,
            update_base_url: None,
            channel: None,
            ring: None,
            worker_install_dir: None,
            worker_service_name: None,
            supervisor_service_name: None,
            startup_jitter_secs: None,
            update_interval_secs: None,
            monitor_interval_secs: None,
        };
        let mut iter = raw_args.iter();
        while let Some(arg) = iter.next() {
            match arg.as_str() {
                "--once" => args.once = true,
                "--update-base-url" => {
                    args.update_base_url = iter.next().map(|value| value.to_string())
                }
                "--channel" => args.channel = iter.next().map(|value| value.to_string()),
                "--ring" => args.ring = iter.next().map(|value| value.to_string()),
                "--worker-install-dir" => {
                    args.worker_install_dir = iter.next().map(|value| PathBuf::from(value.as_str()))
                }
                "--worker-service-name" => {
                    args.worker_service_name = iter.next().map(|value| value.to_string())
                }
                "--supervisor-service-name" => {
                    args.supervisor_service_name = iter.next().map(|value| value.to_string())
                }
                "--startup-jitter-secs" => {
                    args.startup_jitter_secs = iter.next().and_then(|value| value.parse().ok())
                }
                "--update-interval-secs" => {
                    args.update_interval_secs = iter.next().and_then(|value| value.parse().ok())
                }
                "--monitor-interval-secs" => {
                    args.monitor_interval_secs = iter.next().and_then(|value| value.parse().ok())
                }
                other => bail!("unknown supervisor arg: {other}"),
            }
        }
        Ok(args)
    }
}

pub(crate) fn run(args: SupervisorArgs) -> Result<()> {
    run_with_shutdown(args, Arc::new(AtomicBool::new(false)))
}

pub(crate) fn run_with_shutdown(
    args: SupervisorArgs,
    shutting_down: Arc<AtomicBool>,
) -> Result<()> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("build supervisor runtime")?;
    runtime.block_on(run_async(args, shutting_down))
}

async fn run_async(args: SupervisorArgs, shutting_down: Arc<AtomicBool>) -> Result<()> {
    let config = SupervisorConfig::from_args(args)?;
    if let Some(update_base_url) = config.update_base_url.as_deref() {
        log_embedded_manifest_key(update_base_url);
    } else {
        info!(
            "Talos automatic updates are disabled; configure RMM_UPDATE_BASE_URL to use a self-hosted update API"
        );
    }
    if let Err(err) = promote_pending_supervisor(&config.supervisor_install_dir) {
        warn!(
            error = %err,
            install_dir = %config.supervisor_install_dir.display(),
            "pending Talos Supervisor self-update could not be promoted"
        );
    }
    fs::create_dir_all(&config.worker_install_dir)
        .with_context(|| format!("create {}", config.worker_install_dir.display()))?;
    if let Some(parent) = config.worker_version_path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }

    info!(
        update_base_url = ?config.update_base_url,
        channel = %config.channel,
        ring = ?config.ring,
        worker_arch = %config.worker_arch,
        worker_install_dir = %config.worker_install_dir.display(),
        worker_environment_file = ?config.worker_environment_file,
        worker_version_path = %config.worker_version_path.display(),
        worker_service_name = %config.worker_service_name,
        supervisor_service_name = %config.supervisor_service_name,
        "Talos Supervisor configured"
    );

    if let Err(err) = ensure_worker_service_healthy(&config) {
        warn!(error = %err, "Talos Worker service is not healthy before startup update cycle");
    }
    #[cfg(target_os = "macos")]
    if let Err(err) = restart_worker_if_permissions_helper_requested(&config) {
        warn!(error = %err, "Talos Worker permission-helper restart request failed before startup update cycle");
    }

    let startup_delay = jitter_secs(config.startup_jitter_secs);
    if startup_delay > 0 {
        info!(
            startup_delay_secs = startup_delay,
            "Talos Supervisor startup update jitter"
        );
        tokio::select! {
            _ = wait_for_shutdown(&shutting_down) => {
                info!("Talos Supervisor shutdown requested during startup jitter");
                return Ok(());
            }
            _ = tokio::time::sleep(Duration::from_secs(startup_delay)) => {}
        }
    }

    if !run_update_cycle_until_shutdown(&config, "startup", &shutting_down).await {
        info!("Talos Supervisor shutdown requested during startup update cycle");
        return Ok(());
    }
    if let Err(err) = ensure_worker_service_healthy(&config) {
        warn!(error = %err, "Talos Worker service is not healthy after startup update cycle");
    }
    #[cfg(target_os = "macos")]
    if let Err(err) = restart_worker_if_permissions_helper_requested(&config) {
        warn!(error = %err, "Talos Worker permission-helper restart request failed");
    }
    if config.once {
        return Ok(());
    }

    let mut last_update_check = Instant::now();
    let mut last_health_check = Instant::now();
    let idle_tick = Duration::from_secs(1);
    loop {
        if shutting_down.load(Ordering::SeqCst) {
            break;
        }
        tokio::select! {
            _ = wait_for_shutdown(&shutting_down) => break,
            _ = tokio::time::sleep(idle_tick) => {}
        }
        if last_update_check.elapsed() >= config.update_interval {
            if !run_update_cycle_until_shutdown(&config, "periodic", &shutting_down).await {
                break;
            }
            last_update_check = Instant::now();
        }
        if last_health_check.elapsed() >= config.monitor_interval {
            if let Err(err) = ensure_worker_service_healthy(&config) {
                warn!(error = %err, "Talos Worker service health repair failed");
            }
            #[cfg(target_os = "macos")]
            if let Err(err) = restart_worker_if_permissions_helper_requested(&config) {
                warn!(error = %err, "Talos Worker permission-helper restart request failed");
            }
            last_health_check = Instant::now();
        }
    }
    info!("Talos Supervisor shutdown requested");
    Ok(())
}

impl SupervisorConfig {
    fn from_args(args: SupervisorArgs) -> Result<Self> {
        let client = Client::builder()
            .connect_timeout(UPDATE_HTTP_CONNECT_TIMEOUT)
            .timeout(UPDATE_HTTP_TIMEOUT)
            .user_agent(format!("TalosSupervisor/{}", env!("CARGO_PKG_VERSION")))
            .build()
            .context("build update client")?;
        let worker_arch = detect_worker_arch();
        let supervisor_install_dir = current_install_dir()?;
        let worker_install_dir = args
            .worker_install_dir
            .or_else(|| env_path("RMM_WORKER_INSTALL_DIR"))
            .unwrap_or_else(|| default_worker_install_dir(&worker_arch));
        let worker_version_path = env_path("RMM_WORKER_VERSION_PATH")
            .unwrap_or_else(|| default_worker_version_path(&worker_install_dir));
        Ok(Self {
            client,
            update_base_url: resolve_update_base_url(args.update_base_url),
            channel: resolve_string(args.channel, "RMM_UPDATE_CHANNEL", "stable"),
            ring: args
                .ring
                .or_else(|| env_string("RMM_UPDATE_RING"))
                .filter(|value| !value.trim().is_empty()),
            worker_arch,
            worker_install_dir,
            worker_environment_file: env_path("RMM_WORKER_ENV_FILE")
                .or_else(default_worker_environment_file),
            worker_version_path,
            worker_service_name: args
                .worker_service_name
                .or_else(|| env_string("RMM_WORKER_SERVICE_NAME"))
                .unwrap_or_else(|| WORKER_SERVICE_NAME.to_string()),
            supervisor_service_name: args
                .supervisor_service_name
                .or_else(|| env_string("RMM_SUPERVISOR_SERVICE_NAME"))
                .unwrap_or_else(|| SUPERVISOR_SERVICE_NAME.to_string()),
            supervisor_install_dir,
            startup_jitter_secs: args
                .startup_jitter_secs
                .or_else(|| env_u64("RMM_SUPERVISOR_STARTUP_JITTER_SECS"))
                .unwrap_or(0),
            update_interval: Duration::from_secs(
                args.update_interval_secs
                    .or_else(|| env_u64("RMM_SUPERVISOR_UPDATE_INTERVAL_SECS"))
                    .unwrap_or(24 * 60 * 60)
                    .max(1),
            ),
            monitor_interval: Duration::from_secs(
                args.monitor_interval_secs
                    .or_else(|| env_u64("RMM_SUPERVISOR_MONITOR_INTERVAL_SECS"))
                    .unwrap_or(60)
                    .max(1),
            ),
            once: args.once || env_bool("RMM_SUPERVISOR_ONCE"),
        })
    }
}

async fn run_update_cycle(config: &SupervisorConfig, reason: &'static str) {
    let Some(update_base_url) = config.update_base_url.as_deref() else {
        debug!(
            reason,
            "Talos automatic update check skipped because updates are disabled"
        );
        return;
    };
    let supervisor_update_launched =
        match check_supervisor_update(config, update_base_url, reason).await {
            Ok(launched) => launched,
            Err(err) => {
                warn!(
                    error = %format_error_chain(&err),
                    reason,
                    "Talos Supervisor update check failed"
                );
                false
            }
        };
    if supervisor_update_launched {
        info!(
            reason,
            "Talos Supervisor update launched; deferring Worker update check until refreshed supervisor resumes"
        );
        return;
    }
    if let Err(err) = check_worker_update(config, update_base_url, reason).await {
        warn!(
            error = %format_error_chain(&err),
            reason,
            "Talos Worker update check failed"
        );
    }
}

async fn run_update_cycle_until_shutdown(
    config: &SupervisorConfig,
    reason: &'static str,
    shutting_down: &AtomicBool,
) -> bool {
    tokio::select! {
        _ = wait_for_shutdown(shutting_down) => false,
        _ = run_update_cycle(config, reason) => true,
    }
}

async fn wait_for_shutdown(shutting_down: &AtomicBool) {
    while !shutting_down.load(Ordering::SeqCst) {
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

async fn check_supervisor_update(
    config: &SupervisorConfig,
    update_base_url: &str,
    reason: &'static str,
) -> Result<bool> {
    let current_version = env!("CARGO_PKG_VERSION");
    let arch = supervisor_arch();
    let Some(update) = fetch_update(
        config,
        update_base_url,
        SUPERVISOR_PRODUCT,
        arch,
        current_version,
        reason,
    )
    .await?
    else {
        return Ok(false);
    };
    let supervisor_path = env::current_exe().context("resolve current supervisor exe")?;
    let apply_helper_path = prepare_supervisor_apply_helper(&supervisor_path)?;
    launch_supervisor_apply_helper(config, &apply_helper_path, &update)?;
    info!(
        version = %update.version,
        package_path = %update.package_path.display(),
        apply_helper_path = %apply_helper_path.display(),
        "Talos Supervisor self-update launched"
    );
    Ok(true)
}

fn prepare_supervisor_apply_helper(supervisor_path: &Path) -> Result<PathBuf> {
    let helper_dir = update_root_dir().join("supervisor");
    fs::create_dir_all(&helper_dir).with_context(|| format!("create {}", helper_dir.display()))?;
    let helper_path = helper_dir.join(supervisor_apply_helper_name());
    let _ = fs::remove_file(&helper_path);
    fs::copy(supervisor_path, &helper_path).with_context(|| {
        format!(
            "copy supervisor apply helper {} -> {}",
            supervisor_path.display(),
            helper_path.display()
        )
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(&helper_path)
            .with_context(|| format!("metadata {}", helper_path.display()))?
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&helper_path, permissions)
            .with_context(|| format!("chmod 0755 {}", helper_path.display()))?;
    }
    Ok(helper_path)
}

fn supervisor_apply_helper_name() -> String {
    #[cfg(target_os = "windows")]
    {
        format!("talos_supervisor.apply.{}.exe", std::process::id())
    }
    #[cfg(not(target_os = "windows"))]
    {
        format!("talos_supervisor.apply.{}", std::process::id())
    }
}

fn launch_supervisor_apply_helper(
    config: &SupervisorConfig,
    helper_path: &Path,
    update: &DownloadedUpdate,
) -> Result<()> {
    let args = vec![
        "--apply".to_string(),
        "--package".to_string(),
        update.package_path.display().to_string(),
        "--install-dir".to_string(),
        config.supervisor_install_dir.display().to_string(),
        "--service-name".to_string(),
        config.supervisor_service_name.clone(),
        "--target-version".to_string(),
        update.version.clone(),
    ];
    #[cfg(target_os = "linux")]
    {
        if command_available("systemd-run") {
            let unit_name = format!("talos-supervisor-apply-{}", std::process::id());
            let mut systemd_args = vec![
                "--quiet".to_string(),
                "--collect".to_string(),
                "--unit".to_string(),
                unit_name,
                helper_path.display().to_string(),
            ];
            systemd_args.extend(args.iter().cloned());
            let status = Command::new("systemd-run")
                .args(&systemd_args)
                .status()
                .context("launch supervisor apply helper with systemd-run")?;
            if !status.success() {
                bail!(
                    "systemd-run failed to launch supervisor apply helper with code {:?}",
                    status.code()
                );
            }
            return Ok(());
        }
    }
    Command::new(helper_path)
        .args(args)
        .spawn()
        .with_context(|| format!("launch {}", helper_path.display()))?;
    Ok(())
}

async fn check_worker_update(
    config: &SupervisorConfig,
    update_base_url: &str,
    reason: &'static str,
) -> Result<()> {
    let current_version = current_worker_version(config);
    let worker_missing = resolve_worker_exe(&config.worker_install_dir).is_none();
    let current_version_for_manifest = if worker_missing {
        "0.0.0"
    } else {
        current_version.as_deref().unwrap_or("0.0.0")
    };
    let arch_candidates = worker_arch_candidates(&config.worker_arch);
    let mut last_error = None;
    let mut selected_update = None;
    for (index, arch) in arch_candidates.iter().enumerate() {
        match fetch_update(
            config,
            update_base_url,
            WORKER_PRODUCT,
            arch,
            current_version_for_manifest,
            reason,
        )
        .await
        {
            Ok(update) => {
                selected_update = update;
                break;
            }
            Err(err) => {
                warn!(
                    error = %format_error_chain(&err),
                    arch = %arch,
                    fallback_remaining = index + 1 < arch_candidates.len(),
                    "Talos Worker update manifest unavailable for architecture"
                );
                last_error = Some(err);
            }
        }
    }
    let Some(update) = selected_update else {
        if let Some(err) = last_error {
            return Err(err);
        }
        return Ok(());
    };
    let args = ApplyArgs {
        package_path: update.package_path,
        install_dir: config.worker_install_dir.clone(),
        service_name: config.worker_service_name.clone(),
        target_version: Some(update.version.clone()),
    };
    apply_update_package(&args)?;
    write_worker_version(config, &update.version)?;
    ensure_worker_service_healthy(config)?;
    info!(version = %update.version, "Talos Worker update applied");
    Ok(())
}

struct DownloadedUpdate {
    version: String,
    package_path: PathBuf,
}

async fn fetch_update(
    config: &SupervisorConfig,
    update_base_url: &str,
    product: &str,
    arch: &str,
    current_version: &str,
    reason: &'static str,
) -> Result<Option<DownloadedUpdate>> {
    let manifest_url = build_manifest_url(config, update_base_url, product, arch, current_version);
    let fetch_result =
        fetch_manifest_with_retry(config, product, arch, reason, &manifest_url).await?;
    let signed = match fetch_result {
        ManifestFetchResult::NoUpdate | ManifestFetchResult::NotModified => return Ok(None),
        ManifestFetchResult::Signed(signed) => signed,
    };
    verify_manifest_signature(
        EMBEDDED_MANIFEST_PUBLIC_KEY_DER,
        &signed.manifest_bytes,
        &signed.signature_b64,
    )?;
    let expected_manifest = UpdateManifestExpectation::for_artifact(
        product,
        arch,
        &config.channel,
        config.ring.as_deref(),
    )?;
    validate_manifest_context(&signed.manifest, &expected_manifest)
        .with_context(|| format!("{product} update manifest context verification failed"))?;
    if !is_update_newer(current_version, &signed.manifest.version)? {
        debug!(
            product,
            arch,
            reason,
            current_version,
            manifest_version = %signed.manifest.version,
            "manifest not newer than installed version"
        );
        return Ok(None);
    }
    let package_url = build_package_url(
        update_base_url,
        product,
        arch,
        &config.channel,
        config.ring.as_deref(),
    );
    let package_path = update_download_path(product, arch, &signed.manifest.version);
    download_file_with_retry(
        config,
        product,
        arch,
        reason,
        &package_url,
        &package_path,
        signed.manifest.package.size_bytes,
    )
    .await?;
    let actual_size = fs::metadata(&package_path)
        .with_context(|| format!("inspect {}", package_path.display()))?
        .len();
    verify_package_size(signed.manifest.package.size_bytes, actual_size)
        .with_context(|| format!("{product} update package size verification failed"))?;
    let actual_hash = sha256_hex_file(&package_path)?;
    verify_package_sha256(&signed.manifest.package.sha256, &actual_hash)
        .with_context(|| format!("{product} update package digest verification failed"))?;
    info!(
        product,
        arch,
        reason,
        version = %signed.manifest.version,
        package_path = %package_path.display(),
        "downloaded Talos update package"
    );
    Ok(Some(DownloadedUpdate {
        version: signed.manifest.version,
        package_path,
    }))
}

async fn fetch_manifest_with_retry(
    config: &SupervisorConfig,
    product: &str,
    arch: &str,
    reason: &'static str,
    manifest_url: &str,
) -> Result<ManifestFetchResult> {
    let mut delay = Duration::from_secs(2);
    for attempt in 1..=UPDATE_REQUEST_ATTEMPTS {
        match fetch_manifest(&config.client, manifest_url, None).await {
            Ok(result) => return Ok(result),
            Err(err) if attempt < UPDATE_REQUEST_ATTEMPTS => {
                warn!(
                    error = %format_error_chain(&err),
                    product,
                    arch,
                    reason,
                    attempt,
                    retry_in_ms = delay.as_millis() as u64,
                    "Talos update manifest request failed; retrying"
                );
                tokio::time::sleep(delay).await;
                delay = delay.saturating_mul(2);
            }
            Err(err) => return Err(err),
        }
    }
    unreachable!("update manifest retry loop always returns");
}

async fn download_file_with_retry(
    config: &SupervisorConfig,
    product: &str,
    arch: &str,
    reason: &'static str,
    package_url: &str,
    package_path: &Path,
    expected_size_bytes: u64,
) -> Result<()> {
    let mut delay = Duration::from_secs(2);
    for attempt in 1..=UPDATE_REQUEST_ATTEMPTS {
        match download_file(
            &config.client,
            package_url,
            package_path,
            expected_size_bytes,
        )
        .await
        {
            Ok(()) => return Ok(()),
            Err(err) if attempt < UPDATE_REQUEST_ATTEMPTS => {
                warn!(
                    error = %format_error_chain(&err),
                    product,
                    arch,
                    reason,
                    attempt,
                    retry_in_ms = delay.as_millis() as u64,
                    "Talos update package download failed; retrying"
                );
                tokio::time::sleep(delay).await;
                delay = delay.saturating_mul(2);
            }
            Err(err) => return Err(err),
        }
    }
    unreachable!("update package retry loop always returns");
}

fn format_error_chain(error: &anyhow::Error) -> String {
    format!("{error:#}")
}

fn ensure_worker_service_healthy(config: &SupervisorConfig) -> Result<()> {
    let worker_exe = resolve_worker_exe(&config.worker_install_dir).ok_or_else(|| {
        anyhow!(
            "Talos Worker executable not found in {}",
            config.worker_install_dir.display()
        )
    })?;
    let service_manager = platform_service_manager()?;
    let worker_service = WorkerServiceConfig {
        service_name: config.worker_service_name.clone(),
        display_name: "Talos Worker".to_string(),
        description: "Talos remote monitoring and management worker".to_string(),
        executable_path: worker_exe.clone(),
        environment_file: config.worker_environment_file.clone(),
    };
    let state = service_manager.service_state(&config.worker_service_name)?;
    let service_changed = service_manager.ensure_worker_service(&worker_service)?;
    service_manager.ensure_worker_firewall_rule(&worker_exe);
    match state {
        Some(ServiceState::Running) if service_changed => {
            info!(
                service_name = %config.worker_service_name,
                "Talos Worker service unit changed; restarting service"
            );
            service_manager.stop_service(&config.worker_service_name)?;
            service_manager.start_service(&config.worker_service_name)
        }
        Some(ServiceState::Running) => Ok(()),
        Some(state) => {
            info!(
                service_name = %config.worker_service_name,
                ?state,
                "Talos Worker service is not running; starting"
            );
            service_manager.start_service(&config.worker_service_name)
        }
        None => service_manager.start_service(&config.worker_service_name),
    }
}

fn resolve_worker_exe(worker_install_dir: &Path) -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        let path = worker_install_dir.join(MACOS_WORKER_APP_EXE_RELATIVE_PATH);
        return path.exists().then_some(path);
    }
    #[cfg(not(target_os = "macos"))]
    {
        for file_name in [WORKER_EXE_NAME, LEGACY_WORKER_EXE_NAME] {
            let path = worker_install_dir.join(file_name);
            if path.exists() {
                return Some(path);
            }
        }
        None
    }
}

#[cfg(target_os = "macos")]
fn restart_worker_if_permissions_helper_requested(config: &SupervisorConfig) -> Result<()> {
    let request_path = Path::new(MACOS_WORKER_RESTART_REQUEST_PATH);
    if !request_path.exists() {
        return Ok(());
    }
    if !macos_worker_full_disk_access_granted(config) {
        info!(
            request_path = %request_path.display(),
            "permission-helper requested worker restart, but Full Disk Access is not granted yet"
        );
        return Ok(());
    }
    let _ = fs::remove_file(request_path);
    let service_manager = platform_service_manager()?;
    info!(
        service_name = %config.worker_service_name,
        "permission-helper requested Talos Worker restart after Full Disk Access grant"
    );
    let _ = service_manager.stop_service(&config.worker_service_name);
    service_manager.start_service(&config.worker_service_name)
}

#[cfg(target_os = "macos")]
fn macos_worker_full_disk_access_granted(config: &SupervisorConfig) -> bool {
    let Some(worker_exe) = resolve_worker_exe(&config.worker_install_dir) else {
        return false;
    };
    let Ok(output) = Command::new(worker_exe)
        .arg("--check-full-disk-access")
        .arg("--json")
        .output()
    else {
        return false;
    };
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(&output.stdout) else {
        return false;
    };
    value
        .get("granted")
        .and_then(|value| value.as_bool())
        .unwrap_or(false)
}

fn current_worker_version(config: &SupervisorConfig) -> Option<String> {
    env_string("RMM_WORKER_CURRENT_VERSION").or_else(|| {
        fs::read_to_string(&config.worker_version_path)
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
    })
}

fn write_worker_version(config: &SupervisorConfig, version: &str) -> Result<()> {
    if let Some(parent) = config.worker_version_path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    fs::write(&config.worker_version_path, version).context("write worker version")
}

fn build_manifest_url(
    config: &SupervisorConfig,
    update_base_url: &str,
    product: &str,
    arch: &str,
    current_version: &str,
) -> String {
    let mut url = format!(
        "{}/{product}/manifest?arch={arch}&channel={}&currentVersion={current_version}&rolloutSeed={}",
        normalize_base_url(update_base_url),
        config.channel,
        rollout_seed()
    );
    if let Some(ring) = config.ring.as_ref() {
        url.push_str("&ring=");
        url.push_str(ring);
    }
    url
}

fn build_package_url(
    update_base_url: &str,
    product: &str,
    arch: &str,
    channel: &str,
    ring: Option<&str>,
) -> String {
    let mut url = format!(
        "{}/{product}/package?arch={arch}&channel={channel}",
        normalize_base_url(update_base_url)
    );
    if let Some(ring) = ring {
        url.push_str("&ring=");
        url.push_str(ring);
    }
    url
}

fn normalize_base_url(base_url: &str) -> String {
    base_url.trim_end_matches('/').to_string()
}

fn update_download_path(product: &str, arch: &str, version: &str) -> PathBuf {
    update_root_dir()
        .join(product)
        .join(format!("{product}-{arch}-{version}.zip"))
}

fn update_root_dir() -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        if let Ok(base) = env::var("PROGRAMDATA") {
            return PathBuf::from(base).join("Talos").join("updates");
        }
        env::temp_dir().join("Talos").join("updates")
    }
    #[cfg(not(target_os = "windows"))]
    {
        env_path("RMM_SUPERVISOR_UPDATE_ROOT").unwrap_or_else(|| {
            if cfg!(target_os = "macos") {
                PathBuf::from("/Library/Application Support/Talos").join("updates")
            } else {
                PathBuf::from("/var/lib/talos/updates")
            }
        })
    }
}

fn current_install_dir() -> Result<PathBuf> {
    let exe = env::current_exe().context("resolve current supervisor exe")?;
    #[cfg(target_os = "macos")]
    if let Some(install_dir) = macos_app_install_dir(&exe) {
        return Ok(install_dir);
    }
    exe.parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| anyhow!("supervisor install directory not found"))
}

#[cfg(target_os = "macos")]
fn macos_app_install_dir(exe: &Path) -> Option<PathBuf> {
    let macos_dir = exe.parent()?;
    if macos_dir.file_name().and_then(|value| value.to_str()) != Some("MacOS") {
        return None;
    }
    let contents_dir = macos_dir.parent()?;
    if contents_dir.file_name().and_then(|value| value.to_str()) != Some("Contents") {
        return None;
    }
    let app_dir = contents_dir.parent()?;
    if !app_dir
        .extension()
        .and_then(|value| value.to_str())
        .is_some_and(|value| value.eq_ignore_ascii_case("app"))
    {
        return None;
    }
    app_dir.parent().map(Path::to_path_buf)
}

fn resolve_update_base_url(cli_value: Option<String>) -> Option<String> {
    let configured_update_base = env_string("RMM_UPDATE_BASE_URL");
    let api_backend_base = env_string("API_BACKEND_URL");
    let internal_api_base = env_string("INTERNAL_API_URL");
    resolve_update_base_url_from_values([
        cli_value.as_deref(),
        configured_update_base.as_deref(),
        api_backend_base.as_deref(),
        internal_api_base.as_deref(),
    ])
}

fn resolve_update_base_url_from_values(values: [Option<&str>; 4]) -> Option<String> {
    values
        .into_iter()
        .flatten()
        .find(|value| !value.trim().is_empty())
        .and_then(normalize_update_base_url)
}

fn default_worker_install_dir(worker_arch: &str) -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        let base = if worker_arch == "x86" {
            env::var("ProgramFiles(x86)")
                .or_else(|_| env::var("ProgramFiles"))
                .unwrap_or_else(|_| r"C:\Program Files (x86)".to_string())
        } else {
            env::var("ProgramW6432")
                .or_else(|_| env::var("ProgramFiles"))
                .unwrap_or_else(|_| r"C:\Program Files".to_string())
        };
        PathBuf::from(base).join("Talos").join("Worker")
    }
    #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
    {
        let _ = worker_arch;
        PathBuf::from("/opt/talos/worker")
    }
    #[cfg(target_os = "macos")]
    {
        let _ = worker_arch;
        PathBuf::from("/Library/Talos/Worker")
    }
}

fn default_worker_version_path(worker_install_dir: &Path) -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        worker_install_dir.join(WORKER_VERSION_FILE_NAME)
    }
    #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
    {
        let _ = worker_install_dir;
        PathBuf::from("/var/lib/talos").join(WORKER_VERSION_FILE_NAME)
    }
    #[cfg(target_os = "macos")]
    {
        let _ = worker_install_dir;
        PathBuf::from("/Library/Application Support/Talos").join(WORKER_VERSION_FILE_NAME)
    }
}

fn default_worker_environment_file() -> Option<PathBuf> {
    #[cfg(target_os = "linux")]
    {
        Some(PathBuf::from("/etc/talos/rmm-agent.env"))
    }
    #[cfg(target_os = "macos")]
    {
        Some(PathBuf::from("/Library/Preferences/Talos/rmm-agent.env"))
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        None
    }
}

fn detect_worker_arch() -> String {
    #[cfg(target_os = "windows")]
    {
        if !is_64_bit_windows() {
            return "x86".to_string();
        }
        if supports_x64_v4() {
            "x64-v4".to_string()
        } else if supports_x64_v3() {
            "x64-v3".to_string()
        } else if supports_x64_v2() {
            "x64-v2".to_string()
        } else {
            "x64-v1".to_string()
        }
    }
    #[cfg(target_os = "linux")]
    {
        linux_arch()
    }
    #[cfg(target_os = "macos")]
    {
        macos_arch()
    }
    #[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
    {
        format!("{}-{}", env::consts::OS, env::consts::ARCH)
    }
}

#[cfg(target_os = "linux")]
fn linux_arch() -> String {
    match env::consts::ARCH {
        "x86_64" => "linux-x64".to_string(),
        "aarch64" => "linux-arm64".to_string(),
        "x86" => "linux-x86".to_string(),
        "arm" => "linux-arm".to_string(),
        other => format!("linux-{other}"),
    }
}

#[cfg(target_os = "macos")]
fn macos_arch() -> String {
    match env::consts::ARCH {
        "aarch64" => "macos-arm64".to_string(),
        "x86_64" => "macos-x64".to_string(),
        other => format!("macos-{other}"),
    }
}

fn worker_arch_candidates(detected: &str) -> Vec<String> {
    #[cfg(target_os = "windows")]
    {
        let order = ["x64-v4", "x64-v3", "x64-v2", "x64-v1", "x64", "x86"];
        if detected == "x86" {
            return vec!["x86".to_string()];
        }
        let start = order.iter().position(|arch| *arch == detected).unwrap_or(3);
        order[start..order.len() - 1]
            .iter()
            .map(|arch| (*arch).to_string())
            .collect()
    }
    #[cfg(not(target_os = "windows"))]
    {
        vec![detected.to_string()]
    }
}

fn supervisor_arch() -> &'static str {
    #[cfg(target_os = "windows")]
    {
        if cfg!(target_pointer_width = "64") {
            "x64"
        } else {
            "x86"
        }
    }
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    {
        "linux-x64"
    }
    #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
    {
        "linux-arm64"
    }
    #[cfg(all(target_os = "linux", target_arch = "x86"))]
    {
        "linux-x86"
    }
    #[cfg(all(target_os = "linux", target_arch = "arm"))]
    {
        "linux-arm"
    }
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        "macos-arm64"
    }
    #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
    {
        "macos-x64"
    }
    #[cfg(not(any(
        target_os = "windows",
        all(target_os = "linux", target_arch = "x86_64"),
        all(target_os = "linux", target_arch = "aarch64"),
        all(target_os = "linux", target_arch = "x86"),
        all(target_os = "linux", target_arch = "arm"),
        all(target_os = "macos", target_arch = "aarch64"),
        all(target_os = "macos", target_arch = "x86_64")
    )))]
    {
        "unknown"
    }
}

#[cfg(target_os = "windows")]
fn is_64_bit_windows() -> bool {
    if cfg!(target_pointer_width = "64") {
        return true;
    }
    env::var("PROCESSOR_ARCHITEW6432")
        .or_else(|_| env::var("PROCESSOR_ARCHITECTURE"))
        .map(|value| {
            let upper = value.to_ascii_uppercase();
            upper.contains("AMD64") || upper.contains("ARM64")
        })
        .unwrap_or(false)
}

#[cfg(all(
    target_os = "windows",
    any(target_arch = "x86", target_arch = "x86_64")
))]
fn supports_x64_v2() -> bool {
    std::is_x86_feature_detected!("sse3")
        && std::is_x86_feature_detected!("ssse3")
        && std::is_x86_feature_detected!("sse4.1")
        && std::is_x86_feature_detected!("sse4.2")
        && std::is_x86_feature_detected!("popcnt")
}

#[cfg(all(
    target_os = "windows",
    not(any(target_arch = "x86", target_arch = "x86_64"))
))]
fn supports_x64_v2() -> bool {
    false
}

#[cfg(all(
    target_os = "windows",
    any(target_arch = "x86", target_arch = "x86_64")
))]
fn supports_x64_v3() -> bool {
    supports_x64_v2()
        && std::is_x86_feature_detected!("avx")
        && std::is_x86_feature_detected!("avx2")
        && std::is_x86_feature_detected!("bmi1")
        && std::is_x86_feature_detected!("bmi2")
        && std::is_x86_feature_detected!("fma")
}

#[cfg(all(
    target_os = "windows",
    not(any(target_arch = "x86", target_arch = "x86_64"))
))]
fn supports_x64_v3() -> bool {
    false
}

#[cfg(all(
    target_os = "windows",
    any(target_arch = "x86", target_arch = "x86_64")
))]
fn supports_x64_v4() -> bool {
    supports_x64_v3()
        && std::is_x86_feature_detected!("avx512f")
        && std::is_x86_feature_detected!("avx512bw")
        && std::is_x86_feature_detected!("avx512cd")
        && std::is_x86_feature_detected!("avx512dq")
        && std::is_x86_feature_detected!("avx512vl")
}

#[cfg(all(
    target_os = "windows",
    not(any(target_arch = "x86", target_arch = "x86_64"))
))]
fn supports_x64_v4() -> bool {
    false
}

fn rollout_seed() -> String {
    env_string("RMM_AGENT_ID")
        .or_else(|| {
            fs::read_to_string("/etc/machine-id")
                .ok()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
        })
        .or_else(|| env_string("HOSTNAME"))
        .or_else(|| env_string("COMPUTERNAME"))
        .unwrap_or_else(|| "talos-supervisor".to_string())
}

fn resolve_string(cli_value: Option<String>, env_name: &str, default_value: &str) -> String {
    cli_value
        .or_else(|| env_string(env_name))
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| default_value.to_string())
}

fn env_string(name: &str) -> Option<String> {
    env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn env_path(name: &str) -> Option<PathBuf> {
    env_string(name).map(PathBuf::from)
}

fn env_u64(name: &str) -> Option<u64> {
    env_string(name).and_then(|value| value.parse().ok())
}

fn env_bool(name: &str) -> bool {
    env_string(name)
        .map(|value| {
            let normalized = value.to_ascii_lowercase();
            !matches!(normalized.as_str(), "0" | "false" | "no" | "off")
        })
        .unwrap_or(false)
}

#[cfg(target_os = "linux")]
fn command_available(name: &str) -> bool {
    if Path::new(name).is_absolute() {
        return Path::new(name).is_file();
    }
    env::var_os("PATH")
        .map(|paths| env::split_paths(&paths).any(|dir| dir.join(name).is_file()))
        .unwrap_or(false)
}

fn jitter_secs(max: u64) -> u64 {
    if max == 0 {
        return 0;
    }
    let seed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    (seed % u128::from(max + 1)) as u64
}

fn log_embedded_manifest_key(base_url: &str) {
    debug!(
        update_base_url = %base_url,
        manifest_public_key_sha256 = %sha256_hex_bytes(EMBEDDED_MANIFEST_PUBLIC_KEY_DER),
        manifest_public_key_bytes = EMBEDDED_MANIFEST_PUBLIC_KEY_DER.len(),
        "Talos Supervisor manifest trust key loaded"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn update_endpoint_is_disabled_without_configuration() {
        assert_eq!(resolve_update_base_url_from_values([None; 4]), None);
    }

    #[test]
    fn update_endpoint_uses_first_nonempty_configured_value() {
        assert_eq!(
            resolve_update_base_url_from_values([
                Some("   "),
                Some("https://updates.example.test/rmm/updates/"),
                Some("https://ignored.example.test"),
                None,
            ]),
            Some("https://updates.example.test/rmm/updates".to_string())
        );
    }

    #[test]
    fn invalid_explicit_endpoint_does_not_fall_through() {
        assert_eq!(
            resolve_update_base_url_from_values([
                Some("file:///tmp/updates"),
                Some("https://fallback.example.test"),
                None,
                None,
            ]),
            None
        );
    }

    #[test]
    fn package_url_carries_the_signed_manifest_audience() {
        assert_eq!(
            build_package_url(
                "https://updates.example.test/rmm/updates/",
                "worker",
                "linux-x64",
                "stable",
                Some("pilot"),
            ),
            "https://updates.example.test/rmm/updates/worker/package?arch=linux-x64&channel=stable&ring=pilot"
        );
    }
}
