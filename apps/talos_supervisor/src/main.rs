#[cfg(target_os = "macos")]
use std::collections::HashSet;

use std::{
    env,
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    sync::OnceLock,
    time::{Instant, SystemTime, UNIX_EPOCH},
};

#[cfg(target_os = "windows")]
use std::{
    ffi::OsString,
    os::windows::ffi::{OsStrExt, OsStringExt},
    ptr::null_mut,
};

use anyhow::{anyhow, bail, Context, Result};
use tracing::{debug, error, info, warn};
use walkdir::WalkDir;
#[cfg(target_os = "windows")]
use windows_sys::Win32::{
    Foundation::{
        CERT_E_CHAINING, CERT_E_EXPIRED, CERT_E_UNTRUSTEDROOT, ERROR_FILE_NOT_FOUND,
        ERROR_MORE_DATA, ERROR_NO_MORE_ITEMS, ERROR_SUCCESS, TRUST_E_EXPLICIT_DISTRUST,
        TRUST_E_NOSIGNATURE, TRUST_E_SUBJECT_NOT_TRUSTED,
    },
    Security::WinTrust::{
        WinVerifyTrust, WINTRUST_ACTION_GENERIC_VERIFY_V2, WINTRUST_DATA, WINTRUST_DATA_0,
        WINTRUST_FILE_INFO, WTD_CHOICE_FILE, WTD_REVOCATION_CHECK_NONE, WTD_REVOKE_NONE,
        WTD_STATEACTION_CLOSE, WTD_STATEACTION_VERIFY, WTD_UICONTEXT_EXECUTE, WTD_UI_NONE,
    },
    System::Registry::{
        RegCloseKey, RegEnumKeyExW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW, HKEY,
        HKEY_LOCAL_MACHINE, KEY_READ, KEY_SET_VALUE, REG_SZ,
    },
};
use zip::ZipArchive;

mod service_manager;
mod supervisor;

#[cfg(target_os = "windows")]
mod supervisor_service;

#[cfg(target_os = "windows")]
const SUPERVISOR_FILE_NAME: &str = "talos_supervisor.exe";
#[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
const SUPERVISOR_FILE_NAME: &str = "talos_supervisor";
#[cfg(target_os = "windows")]
const LEGACY_SUPERVISOR_FILE_NAME: &str = "updater.exe";
#[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
const LEGACY_SUPERVISOR_FILE_NAME: &str = "updater";
#[cfg(target_os = "windows")]
const PENDING_SUPERVISOR_FILE_NAME: &str = "talos_supervisor.next.exe";
#[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
const PENDING_SUPERVISOR_FILE_NAME: &str = "talos_supervisor.next";
#[cfg(target_os = "windows")]
const LEGACY_PENDING_SUPERVISOR_FILE_NAME: &str = "updater.next.exe";
#[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
const LEGACY_PENDING_SUPERVISOR_FILE_NAME: &str = "updater.next";
#[cfg(target_os = "windows")]
const SUPERVISOR_PREVIOUS_FILE_NAME: &str = "talos_supervisor.previous.exe";
#[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
const SUPERVISOR_PREVIOUS_FILE_NAME: &str = "talos_supervisor.previous";
#[cfg(target_os = "windows")]
const LEGACY_SUPERVISOR_PREVIOUS_FILE_NAME: &str = "updater.previous.exe";
#[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
const LEGACY_SUPERVISOR_PREVIOUS_FILE_NAME: &str = "updater.previous";
#[cfg(target_os = "windows")]
const DEFAULT_APPLY_SERVICE_NAME: &str = "RmmAgent";
#[cfg(not(target_os = "windows"))]
const DEFAULT_APPLY_SERVICE_NAME: &str = "talos-worker";
#[cfg(target_os = "macos")]
const PERMISSIONS_HELPER_APP_NAME: &str = "Talos Permissions Helper.app";
#[cfg(target_os = "macos")]
const PERMISSIONS_HELPER_INSTALL_DIR: &str = "/Applications";
static UPDATE_LOG_PATH: OnceLock<PathBuf> = OnceLock::new();

/// Clear `RUST_LOG`; file log level uses `talos_protocol::rmm_tracing_filter_directive` (`RMM_DEBUG`, `RMM_LOGLEVEL`, default `warn`).
fn strip_legacy_log_env_vars() {
    env::remove_var("RUST_LOG");
}

fn main() -> Result<()> {
    strip_legacy_log_env_vars();
    write_bootstrap_log("main_start", None);
    if let Err(err) = init_file_logging() {
        write_bootstrap_log("init_file_logging_err", Some(&err.to_string()));
        return Err(anyhow!("failed to initialize updater file logging: {err}"));
    }
    write_bootstrap_log("init_file_logging_ok", None);

    let raw_args: Vec<String> = env::args().skip(1).collect();
    if ApplyArgs::is_apply_invocation(&raw_args) {
        let args = ApplyArgs::parse(&raw_args)?;
        info!("starting Talos package update");
        debug!(
            package_path = %args.package_path.display(),
            install_dir = %args.install_dir.display(),
            service_name = %args.service_name,
            target_version = ?args.target_version,
            "Talos update args"
        );
        if let Err(err) = apply_update_package(&args) {
            error!(error = %err, "Talos update failed");
            return Err(err);
        }
        info!("Talos update completed successfully");
    } else {
        #[cfg(target_os = "windows")]
        if raw_args.is_empty() {
            match supervisor_service::run() {
                Ok(()) => return Ok(()),
                Err(::windows_service::Error::Winapi(io_err))
                    if io_err.raw_os_error() == Some(1063) =>
                {
                    write_bootstrap_log("service_dispatcher_not_service_context", None);
                }
                Err(err) => {
                    write_bootstrap_log("service_dispatcher_start_err", Some(&err.to_string()));
                    return Err(anyhow!("service dispatcher start failed: {}", err));
                }
            }
        }
        let args = supervisor::SupervisorArgs::parse(&raw_args)?;
        info!("starting Talos Supervisor");
        if let Err(err) = supervisor::run(args) {
            error!(error = %err, "Talos Supervisor failed");
            return Err(err);
        }
    }
    Ok(())
}

pub(crate) struct ApplyArgs {
    pub(crate) package_path: PathBuf,
    pub(crate) install_dir: PathBuf,
    pub(crate) service_name: String,
    pub(crate) target_version: Option<String>,
}

impl ApplyArgs {
    fn is_apply_invocation(raw_args: &[String]) -> bool {
        raw_args
            .iter()
            .any(|arg| arg == "--apply" || arg == "--package")
    }

    fn parse(raw_args: &[String]) -> Result<Self> {
        let mut package_path = None;
        let mut install_dir = None;
        let mut service_name = Some(DEFAULT_APPLY_SERVICE_NAME.to_string());
        let mut target_version = None;
        let mut iter = raw_args.iter();
        while let Some(arg) = iter.next() {
            match arg.as_str() {
                "--apply" => {}
                "--package" => {
                    package_path = iter.next().map(|value| PathBuf::from(value.as_str()))
                }
                "--install-dir" => {
                    install_dir = iter.next().map(|value| PathBuf::from(value.as_str()))
                }
                "--service-name" => service_name = iter.next().map(|value| value.to_string()),
                "--target-version" => target_version = iter.next().map(|value| value.to_string()),
                other => bail!("unknown arg: {other}"),
            }
        }
        Ok(Self {
            package_path: package_path.ok_or_else(|| anyhow!("--package is required"))?,
            install_dir: install_dir.ok_or_else(|| anyhow!("--install-dir is required"))?,
            service_name: service_name.unwrap_or_else(|| DEFAULT_APPLY_SERVICE_NAME.to_string()),
            target_version: target_version
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty()),
        })
    }
}

fn init_file_logging() -> Result<(), std::io::Error> {
    let log_template = updater_log_path();
    let writer = talos_log_util::DailyFileMakeWriter::try_new(log_template.clone())?;
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::new(
            talos_protocol::rmm_tracing_filter_directive(),
        ))
        .with_timer(tracing_subscriber::fmt::time::SystemTime)
        .with_writer(writer)
        .with_ansi(false)
        .init();
    warn!(path = %log_template.display(), "logging to file");
    Ok(())
}

#[cfg(target_os = "windows")]
fn log_path_candidates() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Ok(base) = env::var("PROGRAMDATA") {
        paths.push(
            PathBuf::from(base)
                .join("Talos")
                .join("logs")
                .join("talos_supervisor.log"),
        );
    }
    paths.push(PathBuf::from(
        r"C:\ProgramData\Talos\logs\talos_supervisor.log",
    ));
    paths.push(env::temp_dir().join("talos_supervisor.log"));
    paths.push(PathBuf::from(r"C:\Windows\Temp\talos_supervisor.log"));
    paths
}

#[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
fn log_path_candidates() -> Vec<PathBuf> {
    vec![
        PathBuf::from("/var/log/talos").join("talos_supervisor.log"),
        env::temp_dir().join("talos_supervisor.log"),
    ]
}

#[cfg(target_os = "macos")]
fn log_path_candidates() -> Vec<PathBuf> {
    vec![
        PathBuf::from("/Library/Logs/Talos").join("talos_supervisor.log"),
        env::temp_dir().join("talos_supervisor.log"),
    ]
}

fn resolve_log_path() -> PathBuf {
    for template in log_path_candidates() {
        let Some(parent) = template.parent() else {
            continue;
        };
        if fs::create_dir_all(parent).is_err() {
            continue;
        }
        let probe = parent.join(".talos_log_write_probe");
        if OpenOptions::new()
            .create(true)
            .append(true)
            .open(&probe)
            .is_ok()
        {
            let _ = fs::remove_file(&probe);
            return template;
        }
    }
    log_path_candidates()
        .into_iter()
        .next()
        .unwrap_or_else(|| env::temp_dir().join("talos_supervisor.log"))
}

fn updater_log_path() -> PathBuf {
    UPDATE_LOG_PATH.get_or_init(resolve_log_path).clone()
}

fn write_bootstrap_log(event: &str, data: Option<&str>) {
    let log_template = updater_log_path();
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let line = match data {
        Some(value) => format!("{}  INFO bootstrap: {} {}\n", ts, event, value),
        None => format!("{}  INFO bootstrap: {}\n", ts, event),
    };

    if let Ok(mut file) = talos_log_util::open_today_log_append(&log_template) {
        let _ = file.write_all(line.as_bytes());
        let _ = file.flush();
    } else {
        eprintln!("{}", line.trim_end());
    }
}

pub(crate) fn apply_update_package(args: &ApplyArgs) -> Result<()> {
    let staging_dir = args.install_dir.join("_update_staging");
    if staging_dir.exists() {
        debug!(path = %staging_dir.display(), "removing previous update staging directory");
        if let Err(err) = fs::remove_dir_all(&staging_dir) {
            warn!(error = %err, path = %staging_dir.display(), "failed to remove stale staging directory");
        }
    }
    fs::create_dir_all(&staging_dir)
        .with_context(|| format!("create {}", staging_dir.display()))?;
    debug!(path = %staging_dir.display(), "created update staging directory");
    extract_zip(&args.package_path, &staging_dir)?;

    let restart_service = service_exists(&args.service_name);
    if restart_service {
        stop_service(&args.service_name)?;
    } else {
        debug!(
            service_name = %args.service_name,
            "service does not exist yet; applying files without restart"
        );
    }
    replace_directory_contents(&staging_dir, &args.install_dir)?;
    if restart_service {
        start_service(&args.service_name)?;
    }
    if let Some(version) = args.target_version.as_deref().filter(|_| {
        !args
            .service_name
            .eq_ignore_ascii_case(supervisor::WORKER_SERVICE_NAME_FOR_APPLY)
    }) {
        if let Err(err) = update_arp_display_version(version) {
            warn!(error = %err, version, "failed to update ARP display version");
        }
    }
    if let Err(err) = fs::remove_dir_all(&staging_dir) {
        warn!(error = %err, path = %staging_dir.display(), "failed to remove staging directory after update");
    } else {
        debug!(path = %staging_dir.display(), "removed update staging directory");
    }
    Ok(())
}

fn extract_zip(zip_path: &Path, destination: &Path) -> Result<()> {
    let t0 = Instant::now();
    debug!(
        package_path = %zip_path.display(),
        destination = %destination.display(),
        "extracting agent update package"
    );
    let file = File::open(zip_path).with_context(|| format!("open {}", zip_path.display()))?;
    let mut archive = ZipArchive::new(file).context("open zip archive")?;
    let entry_count = archive.len();
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).context("read zip entry")?;
        let out_path = destination.join(entry.mangled_name());
        if entry.is_dir() {
            fs::create_dir_all(&out_path)
                .with_context(|| format!("create {}", out_path.display()))?;
            continue;
        }
        if let Some(parent) = out_path.parent() {
            fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
        }
        let mut out_file =
            File::create(&out_path).with_context(|| format!("create {}", out_path.display()))?;
        io::copy(&mut entry, &mut out_file)
            .with_context(|| format!("extract {}", out_path.display()))?;
        debug!(path = %out_path.display(), index, "extracted update zip entry");
    }
    debug!(
        destination = %destination.display(),
        entry_count,
        elapsed_ms = t0.elapsed().as_millis() as u64,
        "finished extracting agent update package"
    );
    Ok(())
}

fn replace_directory_contents(staging_dir: &Path, install_dir: &Path) -> Result<()> {
    let t0 = Instant::now();
    debug!(
        staging_dir = %staging_dir.display(),
        install_dir = %install_dir.display(),
        "replacing installed Talos files from staging"
    );
    #[cfg(target_os = "macos")]
    let macos_replaced_app_roots = replace_macos_app_bundles_if_staged(staging_dir, install_dir)?;
    let mut files_replaced = 0u32;
    for entry in WalkDir::new(staging_dir).min_depth(1) {
        let entry = entry.context("walk staging directory")?;
        if !entry.file_type().is_file() {
            continue;
        }
        let relative = entry
            .path()
            .strip_prefix(staging_dir)
            .with_context(|| format!("strip prefix {}", entry.path().display()))?;
        #[cfg(not(target_os = "macos"))]
        let file_name = relative
            .file_name()
            .and_then(|value| value.to_str())
            .ok_or_else(|| anyhow!("invalid staging file name"))?;
        #[cfg(target_os = "macos")]
        if is_macos_replaced_app_relative_path(relative, &macos_replaced_app_roots) {
            continue;
        }
        let destination = install_dir.join(relative);
        #[cfg(not(target_os = "macos"))]
        {
            if is_supervisor_self_file(file_name) {
                verify_authenticode(entry.path())?;
                if running_from_path(&destination) {
                    let pending = pending_supervisor_path(install_dir, file_name);
                    debug!(
                        source = %entry.path().display(),
                        destination = %pending.display(),
                        "staging pending supervisor self-update"
                    );
                    fs::copy(entry.path(), &pending)
                        .with_context(|| format!("copy {}", pending.display()))?;
                    apply_platform_file_permissions(&pending)?;
                } else {
                    let backup = supervisor_backup_path(install_dir, file_name);
                    debug!(
                        source = %entry.path().display(),
                        destination = %destination.display(),
                        backup = %backup.display(),
                        "replacing supervisor executable from detached updater"
                    );
                    replace_file_with_backup(entry.path(), &destination, &backup)?;
                    apply_platform_file_permissions(&destination)?;
                }
                files_replaced += 1;
                continue;
            }
        }
        if destination
            .extension()
            .and_then(|value| value.to_str())
            .map(|value| value.eq_ignore_ascii_case("exe"))
            .unwrap_or(false)
        {
            verify_authenticode(entry.path())?;
        }
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
        }
        let backup_path = destination.with_extension("previous");
        if destination.exists() {
            let _ = fs::remove_file(&backup_path);
            debug!(
                source = %destination.display(),
                backup = %backup_path.display(),
                "creating temporary backup before file replacement"
            );
            fs::rename(&destination, &backup_path).with_context(|| {
                format!(
                    "rename {} -> {}",
                    destination.display(),
                    backup_path.display()
                )
            })?;
        }
        debug!(
            source = %entry.path().display(),
            destination = %destination.display(),
            "replacing installed file"
        );
        fs::copy(entry.path(), &destination)
            .with_context(|| format!("copy {}", destination.display()))?;
        apply_platform_file_permissions(&destination)?;
        let _ = fs::remove_file(&backup_path);
        files_replaced += 1;
    }
    info!(
        files_replaced,
        elapsed_ms = t0.elapsed().as_millis() as u64,
        "finished replacing installed agent files"
    );
    Ok(())
}

#[cfg(target_os = "macos")]
fn replace_macos_app_bundles_if_staged(
    staging_dir: &Path,
    install_dir: &Path,
) -> Result<HashSet<String>> {
    let mut replaced = HashSet::new();
    for entry in
        fs::read_dir(staging_dir).with_context(|| format!("read {}", staging_dir.display()))?
    {
        let entry = entry.context("read staged app bundle entry")?;
        let source = entry.path();
        let is_app_dir = source
            .extension()
            .and_then(|value| value.to_str())
            .is_some_and(|value| value.eq_ignore_ascii_case("app"));
        if !source.is_dir() || !is_app_dir {
            continue;
        }
        let Some(name) = source
            .file_name()
            .and_then(|value| value.to_str())
            .map(|value| value.to_string())
        else {
            continue;
        };
        let destination = macos_app_bundle_destination(install_dir, &name);
        replace_macos_app_bundle(&source, &destination)?;
        replaced.insert(name);
    }
    Ok(replaced)
}

#[cfg(target_os = "macos")]
fn macos_app_bundle_destination(install_dir: &Path, name: &str) -> PathBuf {
    if name == PERMISSIONS_HELPER_APP_NAME {
        PathBuf::from(PERMISSIONS_HELPER_INSTALL_DIR).join(name)
    } else {
        install_dir.join(name)
    }
}

#[cfg(target_os = "macos")]
fn replace_macos_app_bundle(source: &Path, destination: &Path) -> Result<()> {
    let backup = destination.with_extension("app.previous");
    let _ = remove_path_any(&backup);
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    if destination.exists() {
        fs::rename(destination, &backup)
            .with_context(|| format!("rename {} -> {}", destination.display(), backup.display()))?;
    }
    if let Err(err) = copy_directory_tree(source, destination) {
        let _ = remove_path_any(destination);
        if backup.exists() {
            let _ = fs::rename(&backup, destination);
        }
        return Err(err);
    }
    let _ = remove_path_any(&backup);
    info!(
        source = %source.display(),
        destination = %destination.display(),
        "replaced macOS Talos app bundle"
    );
    Ok(())
}

#[cfg(target_os = "macos")]
fn is_macos_replaced_app_relative_path(relative: &Path, replaced: &HashSet<String>) -> bool {
    relative
        .components()
        .next()
        .and_then(|component| match component {
            std::path::Component::Normal(value) => value.to_str(),
            _ => None,
        })
        .is_some_and(|value| replaced.contains(value))
}

#[cfg(target_os = "macos")]
fn copy_directory_tree(source: &Path, destination: &Path) -> Result<()> {
    for entry in WalkDir::new(source).min_depth(0) {
        let entry = entry.context("walk app bundle source")?;
        let relative = entry
            .path()
            .strip_prefix(source)
            .with_context(|| format!("strip prefix {}", entry.path().display()))?;
        let target = destination.join(relative);
        if entry.file_type().is_dir() {
            fs::create_dir_all(&target).with_context(|| format!("create {}", target.display()))?;
            continue;
        }
        if entry.file_type().is_file() {
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)
                    .with_context(|| format!("create {}", parent.display()))?;
            }
            fs::copy(entry.path(), &target).with_context(|| {
                format!("copy {} -> {}", entry.path().display(), target.display())
            })?;
            apply_platform_file_permissions(&target)?;
        }
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn remove_path_any(path: &Path) -> Result<()> {
    if path.is_dir() {
        fs::remove_dir_all(path).with_context(|| format!("remove {}", path.display()))
    } else if path.exists() {
        fs::remove_file(path).with_context(|| format!("remove {}", path.display()))
    } else {
        Ok(())
    }
}

#[cfg(not(target_os = "macos"))]
fn is_supervisor_self_file(file_name: &str) -> bool {
    file_name.eq_ignore_ascii_case(SUPERVISOR_FILE_NAME)
        || file_name.eq_ignore_ascii_case(LEGACY_SUPERVISOR_FILE_NAME)
}

#[cfg(not(target_os = "macos"))]
fn pending_supervisor_path(install_dir: &Path, file_name: &str) -> PathBuf {
    if file_name.eq_ignore_ascii_case(LEGACY_SUPERVISOR_FILE_NAME) {
        install_dir.join(LEGACY_PENDING_SUPERVISOR_FILE_NAME)
    } else {
        install_dir.join(PENDING_SUPERVISOR_FILE_NAME)
    }
}

#[cfg(not(target_os = "macos"))]
fn supervisor_backup_path(install_dir: &Path, file_name: &str) -> PathBuf {
    if file_name.eq_ignore_ascii_case(LEGACY_SUPERVISOR_FILE_NAME) {
        install_dir.join(LEGACY_SUPERVISOR_PREVIOUS_FILE_NAME)
    } else {
        install_dir.join(SUPERVISOR_PREVIOUS_FILE_NAME)
    }
}

#[cfg(not(target_os = "macos"))]
fn running_from_path(path: &Path) -> bool {
    let Ok(current) = env::current_exe() else {
        return false;
    };
    let current_canonical = fs::canonicalize(&current).unwrap_or(current);
    let target_canonical = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    paths_match_for_platform(&current_canonical, &target_canonical)
}

#[cfg(target_os = "windows")]
fn paths_match_for_platform(left: &Path, right: &Path) -> bool {
    left.to_string_lossy()
        .eq_ignore_ascii_case(right.to_string_lossy().as_ref())
}

#[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
fn paths_match_for_platform(left: &Path, right: &Path) -> bool {
    left == right
}

#[cfg(not(target_os = "macos"))]
fn replace_file_with_backup(source: &Path, destination: &Path, backup: &Path) -> Result<()> {
    let _ = fs::remove_file(backup);
    let moved_existing = if destination.exists() {
        fs::rename(destination, backup)
            .with_context(|| format!("rename {} -> {}", destination.display(), backup.display()))?;
        true
    } else {
        false
    };

    if let Err(err) = fs::copy(source, destination) {
        if moved_existing && !destination.exists() && backup.exists() {
            let _ = fs::rename(backup, destination);
        }
        return Err(err).with_context(|| format!("copy {}", destination.display()));
    }
    let _ = fs::remove_file(backup);
    Ok(())
}

#[cfg(target_os = "macos")]
pub(crate) fn promote_pending_supervisor(_install_dir: &Path) -> Result<()> {
    Ok(())
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn promote_pending_supervisor(install_dir: &Path) -> Result<()> {
    promote_pending_supervisor_file(
        &install_dir.join(PENDING_SUPERVISOR_FILE_NAME),
        &install_dir.join(SUPERVISOR_FILE_NAME),
        &install_dir.join(SUPERVISOR_PREVIOUS_FILE_NAME),
    )?;
    promote_pending_supervisor_file(
        &install_dir.join(LEGACY_PENDING_SUPERVISOR_FILE_NAME),
        &install_dir.join(LEGACY_SUPERVISOR_FILE_NAME),
        &install_dir.join(LEGACY_SUPERVISOR_PREVIOUS_FILE_NAME),
    )
}

#[cfg(not(target_os = "macos"))]
fn promote_pending_supervisor_file(pending: &Path, current: &Path, backup: &Path) -> Result<()> {
    if !pending.exists() {
        return Ok(());
    }
    if current.exists() {
        let _ = fs::remove_file(backup);
        fs::rename(current, backup)
            .with_context(|| format!("rename {} -> {}", current.display(), backup.display()))?;
    }
    fs::rename(pending, current)
        .with_context(|| format!("rename {} -> {}", pending.display(), current.display()))?;
    let _ = fs::remove_file(backup);
    Ok(())
}

#[cfg(unix)]
fn apply_platform_file_permissions(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let Some(file_name) = path.file_name().and_then(|value| value.to_str()) else {
        return Ok(());
    };
    if matches!(
        file_name,
        "talos_supervisor"
            | "updater"
            | "talos_worker"
            | "talos-rmm-agent"
            | "talos_worker_helper"
            | "talos_worker_chat"
            | "talos_permissions_helper"
    ) {
        let mut permissions = fs::metadata(path)
            .with_context(|| format!("metadata {}", path.display()))?
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(path, permissions)
            .with_context(|| format!("chmod 0755 {}", path.display()))?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn apply_platform_file_permissions(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(target_os = "windows")]
fn verify_authenticode(path: &Path) -> Result<()> {
    debug!(path = %path.display(), "verifying Authenticode signature with WinVerifyTrust");
    let path_wide = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let mut file_info = WINTRUST_FILE_INFO {
        cbStruct: std::mem::size_of::<WINTRUST_FILE_INFO>() as u32,
        pcwszFilePath: path_wide.as_ptr(),
        hFile: null_mut(),
        pgKnownSubject: null_mut(),
    };
    let mut trust_data = WINTRUST_DATA {
        cbStruct: std::mem::size_of::<WINTRUST_DATA>() as u32,
        pPolicyCallbackData: null_mut(),
        pSIPClientData: null_mut(),
        dwUIChoice: WTD_UI_NONE,
        fdwRevocationChecks: WTD_REVOKE_NONE,
        dwUnionChoice: WTD_CHOICE_FILE,
        Anonymous: WINTRUST_DATA_0 {
            pFile: &mut file_info,
        },
        dwStateAction: WTD_STATEACTION_VERIFY,
        hWVTStateData: null_mut(),
        pwszURLReference: null_mut(),
        dwProvFlags: WTD_REVOCATION_CHECK_NONE,
        dwUIContext: WTD_UICONTEXT_EXECUTE,
        pSignatureSettings: null_mut(),
    };
    let mut action = WINTRUST_ACTION_GENERIC_VERIFY_V2;
    let status = unsafe {
        WinVerifyTrust(
            null_mut(),
            &mut action,
            &mut trust_data as *mut WINTRUST_DATA as *mut core::ffi::c_void,
        )
    };
    trust_data.dwStateAction = WTD_STATEACTION_CLOSE;
    let close_status = unsafe {
        WinVerifyTrust(
            null_mut(),
            &mut action,
            &mut trust_data as *mut WINTRUST_DATA as *mut core::ffi::c_void,
        )
    };
    if close_status != 0 {
        warn!(
            path = %path.display(),
            status = format_wintrust_status(close_status),
            "WinVerifyTrust state cleanup failed"
        );
    }
    if status != 0 {
        bail!(
            "Authenticode verification failed for {} (WinVerifyTrust status: {}, {})",
            path.display(),
            format_wintrust_status(status),
            wintrust_status_description(status)
        );
    }
    debug!(path = %path.display(), "Authenticode signature valid");
    Ok(())
}

#[cfg(target_os = "windows")]
fn format_wintrust_status(status: i32) -> String {
    format!("0x{:08X}", status as u32)
}

#[cfg(target_os = "windows")]
fn wintrust_status_description(status: i32) -> &'static str {
    match status {
        TRUST_E_NOSIGNATURE => "no embedded signature was found",
        CERT_E_EXPIRED => "signing certificate or chain is expired",
        CERT_E_UNTRUSTEDROOT => "signing certificate chain ends in an untrusted root",
        CERT_E_CHAINING => "certificate chain could not be built",
        TRUST_E_EXPLICIT_DISTRUST => "signing certificate is explicitly distrusted",
        TRUST_E_SUBJECT_NOT_TRUSTED => "file is not trusted for this policy",
        _ => "Windows trust provider rejected the file",
    }
}

fn stop_service(service_name: &str) -> Result<()> {
    debug!(service_name, "stopping service for agent update");
    service_manager::platform_service_manager()?.stop_service(service_name)
}

fn service_exists(service_name: &str) -> bool {
    service_manager::platform_service_manager()
        .and_then(|manager| manager.service_exists(service_name))
        .unwrap_or(false)
}

fn start_service(service_name: &str) -> Result<()> {
    service_manager::platform_service_manager()?.start_service(service_name)?;
    info!(service_name, "service restarted after agent update");
    Ok(())
}

#[cfg(target_os = "windows")]
fn update_arp_display_version(version: &str) -> Result<()> {
    const DISPLAY_NAME: &str = "Talos Supervisor";
    const UNINSTALL_ROOTS: [&str; 2] = [
        r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall",
        r"SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall",
    ];

    let version = version.trim();
    if version.is_empty() {
        return Ok(());
    }

    let mut updated = 0usize;
    for root_path in UNINSTALL_ROOTS {
        updated += update_arp_display_version_in_root(root_path, DISPLAY_NAME, version)
            .with_context(|| format!("update ARP display version under HKLM\\{root_path}"))?;
    }

    if updated == 0 {
        bail!("ARP entry not found for {DISPLAY_NAME}");
    }

    debug!(
        version,
        entries_updated = updated,
        display_name = DISPLAY_NAME,
        "updated ARP display version"
    );
    Ok(())
}

#[cfg(not(target_os = "windows"))]
fn verify_authenticode(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(not(target_os = "windows"))]
fn update_arp_display_version(_version: &str) -> Result<()> {
    Ok(())
}

#[cfg(target_os = "windows")]
fn update_arp_display_version_in_root(
    root_path: &str,
    display_name: &str,
    version: &str,
) -> Result<usize> {
    let root = match RegKey::open_hklm(root_path, KEY_READ) {
        Ok(root) => root,
        Err(err) if err == ERROR_FILE_NOT_FOUND => return Ok(0),
        Err(err) => bail!("RegOpenKeyExW failed for HKLM\\{root_path} (winerr {err})"),
    };

    let mut updated = 0usize;
    let mut index = 0u32;
    loop {
        let Some(subkey_name) = enum_registry_subkey(root.hkey, index)
            .with_context(|| format!("enumerate HKLM\\{root_path} subkey {index}"))?
        else {
            break;
        };
        index += 1;

        let subkey_path = format!("{root_path}\\{subkey_name}");
        let subkey = match RegKey::open_hklm(&subkey_path, KEY_READ | KEY_SET_VALUE) {
            Ok(subkey) => subkey,
            Err(err) => {
                debug!(
                    root_path,
                    subkey_name,
                    winerr = err,
                    "skipping ARP subkey that could not be opened"
                );
                continue;
            }
        };

        if query_reg_sz(subkey.hkey, "DisplayName")?.as_deref() == Some(display_name) {
            set_reg_sz(subkey.hkey, "DisplayVersion", version)?;
            updated += 1;
        }
    }

    Ok(updated)
}

#[cfg(target_os = "windows")]
struct RegKey {
    hkey: HKEY,
}

#[cfg(target_os = "windows")]
impl RegKey {
    fn open_hklm(path: &str, access: u32) -> std::result::Result<Self, u32> {
        let path_wide = to_wide_null(path);
        let mut hkey: HKEY = null_mut();
        let status =
            unsafe { RegOpenKeyExW(HKEY_LOCAL_MACHINE, path_wide.as_ptr(), 0, access, &mut hkey) };
        if status != ERROR_SUCCESS {
            return Err(status);
        }
        Ok(Self { hkey })
    }
}

#[cfg(target_os = "windows")]
impl Drop for RegKey {
    fn drop(&mut self) {
        if !self.hkey.is_null() {
            unsafe {
                let _ = RegCloseKey(self.hkey);
            }
        }
    }
}

#[cfg(target_os = "windows")]
fn enum_registry_subkey(hkey: HKEY, index: u32) -> Result<Option<String>> {
    let mut capacity = 256usize;
    loop {
        let mut name = vec![0u16; capacity];
        let mut name_len = name.len() as u32;
        let status = unsafe {
            RegEnumKeyExW(
                hkey,
                index,
                name.as_mut_ptr(),
                &mut name_len,
                null_mut(),
                null_mut(),
                null_mut(),
                null_mut(),
            )
        };
        match status {
            ERROR_SUCCESS => {
                name.truncate(name_len as usize);
                return Ok(Some(
                    OsString::from_wide(&name).to_string_lossy().to_string(),
                ));
            }
            ERROR_NO_MORE_ITEMS => return Ok(None),
            ERROR_MORE_DATA => capacity = capacity.saturating_mul(2),
            other => bail!("RegEnumKeyExW failed (winerr {other})"),
        }
    }
}

#[cfg(target_os = "windows")]
fn query_reg_sz(hkey: HKEY, value_name: &str) -> Result<Option<String>> {
    let value_name_wide = to_wide_null(value_name);
    let mut value_type = 0u32;
    let mut byte_len = 0u32;
    let status = unsafe {
        RegQueryValueExW(
            hkey,
            value_name_wide.as_ptr(),
            null_mut(),
            &mut value_type,
            null_mut(),
            &mut byte_len,
        )
    };
    match status {
        ERROR_SUCCESS => {}
        ERROR_FILE_NOT_FOUND => return Ok(None),
        other => bail!("RegQueryValueExW({value_name}) size failed (winerr {other})"),
    }
    if value_type != REG_SZ {
        return Ok(None);
    }
    if byte_len == 0 {
        return Ok(Some(String::new()));
    }

    let mut bytes = vec![0u8; byte_len as usize];
    let status = unsafe {
        RegQueryValueExW(
            hkey,
            value_name_wide.as_ptr(),
            null_mut(),
            &mut value_type,
            bytes.as_mut_ptr(),
            &mut byte_len,
        )
    };
    if status != ERROR_SUCCESS {
        bail!("RegQueryValueExW({value_name}) data failed (winerr {status})");
    }

    bytes.truncate(byte_len as usize);
    let mut wide = bytes
        .chunks_exact(2)
        .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
        .collect::<Vec<_>>();
    while wide.last().copied() == Some(0) {
        wide.pop();
    }
    Ok(Some(
        OsString::from_wide(&wide).to_string_lossy().to_string(),
    ))
}

#[cfg(target_os = "windows")]
fn set_reg_sz(hkey: HKEY, value_name: &str, value: &str) -> Result<()> {
    let value_name_wide = to_wide_null(value_name);
    let value_wide = to_wide_null(value);
    let bytes = value_wide
        .len()
        .checked_mul(std::mem::size_of::<u16>())
        .context("registry string value length overflow")?;
    let status = unsafe {
        RegSetValueExW(
            hkey,
            value_name_wide.as_ptr(),
            0,
            REG_SZ,
            value_wide.as_ptr().cast::<u8>(),
            bytes
                .try_into()
                .context("registry string value too large to write")?,
        )
    };
    if status != ERROR_SUCCESS {
        bail!("RegSetValueExW({value_name}) failed (winerr {status})");
    }
    Ok(())
}

#[cfg(target_os = "windows")]
fn to_wide_null(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}
