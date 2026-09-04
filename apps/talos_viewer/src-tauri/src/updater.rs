use std::{
    env,
    fs::{self},
    path::{Path, PathBuf},
    process::Command,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, Once,
    },
    time::Duration,
};

use anyhow::{anyhow, Context, Result};
use reqwest::Client;
use serde::Serialize;
#[cfg(windows)]
use std::{
    fs::{File, OpenOptions},
    io::{self, Write},
};
use talos_update_common::{
    download_file, fetch_manifest, is_update_newer, normalize_update_base_url, sha256_hex_bytes,
    sha256_hex_file, validate_manifest_context, verify_manifest_signature, verify_package_sha256,
    verify_package_size, ManifestFetchResult, UpdateManifestExpectation,
};
use tauri::{AppHandle, Emitter, Manager};
use tracing::{debug, info, warn};
#[cfg(windows)]
use zip::ZipArchive;

const EMBEDDED_MANIFEST_PUBLIC_KEY_DER: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/manifest_public_key.der"));
#[cfg(windows)]
const UPDATER_FILE_NAME: &str = "talos_viewer_updater.exe";
#[cfg(windows)]
const PENDING_UPDATER_FILE_NAME: &str = "talos_viewer_updater.next.exe";
const UPDATE_NOTICE_FILE_NAME: &str = ".viewer_update_notice";
#[cfg(windows)]
const WINDOWS_ERROR_ELEVATION_REQUIRED: i32 = 740;
static UPDATE_KEY_LOG_ONCE: Once = Once::new();
static UPDATE_EXIT_NOTIFY: std::sync::OnceLock<tokio::sync::Notify> = std::sync::OnceLock::new();

#[derive(Clone)]
pub struct UpdateManager {
    inner: Arc<Inner>,
}

struct Inner {
    client: Client,
    channel: String,
    ring: Option<String>,
    interval: Duration,
    initial_delay: Duration,
    in_progress: AtomicBool,
    cached_etag: Mutex<Option<String>>,
    pending_package: Mutex<Option<PendingPackage>>,
}

#[derive(Clone)]
struct PendingPackage {
    package_path: PathBuf,
    version: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ManualUpdateCheckResult {
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

enum UpdateApplyMode {
    Automatic,
    Manual,
}

impl UpdateManager {
    pub fn from_env() -> Result<Self> {
        let client = Client::builder()
            .build()
            .context("build viewer update client")?;
        log_embedded_manifest_key();
        let channel = env::var("RMM_VIEWER_UPDATE_CHANNEL")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "stable".to_string());
        let ring = env::var("RMM_VIEWER_UPDATE_RING")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        let interval_secs = env::var("RMM_VIEWER_UPDATE_INTERVAL_SECS")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(6 * 60 * 60);
        let initial_delay_secs = env::var("RMM_VIEWER_UPDATE_INITIAL_DELAY_SECS")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(15 * 60);
        Ok(Self {
            inner: Arc::new(Inner {
                client,
                channel,
                ring,
                interval: Duration::from_secs(interval_secs),
                initial_delay: Duration::from_secs(initial_delay_secs),
                in_progress: AtomicBool::new(false),
                cached_etag: Mutex::new(None),
                pending_package: Mutex::new(None),
            }),
        })
    }

    pub fn start_background_task(&self, app: AppHandle) {
        let manager = self.clone();
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(manager.inner.initial_delay).await;
            loop {
                if let Err(err) = manager
                    .check_once_with_mode(&app, UpdateApplyMode::Automatic)
                    .await
                {
                    warn!(error = %err, "viewer update check failed");
                }
                tokio::time::sleep(manager.inner.interval).await;
            }
        });
    }

    pub async fn manual_check(&self, app: &AppHandle) -> Result<ManualUpdateCheckResult> {
        self.check_once_with_mode(app, UpdateApplyMode::Manual)
            .await
    }

    pub fn apply_staged_update(&self, app: &AppHandle) -> Result<bool> {
        promote_pending_updater()?;
        let pending = self
            .inner
            .pending_package
            .lock()
            .ok()
            .and_then(|guard| guard.clone());
        let Some(pending) = pending else {
            return Ok(false);
        };
        launch_pending_update(&pending.package_path, Some(&pending.version), true)?;
        request_app_exit(app.clone());
        Ok(true)
    }

    async fn check_once_with_mode(
        &self,
        app: &AppHandle,
        apply_mode: UpdateApplyMode,
    ) -> Result<ManualUpdateCheckResult> {
        promote_pending_updater()?;
        if let Some(pending) = self
            .inner
            .pending_package
            .lock()
            .ok()
            .and_then(|guard| guard.clone())
        {
            if matches!(apply_mode, UpdateApplyMode::Automatic) && viewer_is_idle(app) {
                launch_pending_update(&pending.package_path, Some(&pending.version), false)?;
                request_app_exit(app.clone());
                return Ok(ManualUpdateCheckResult {
                    status: "update_ready".to_string(),
                    version: Some(pending.version),
                });
            }
            return Ok(ManualUpdateCheckResult {
                status: "update_ready".to_string(),
                version: Some(pending.version),
            });
        }

        let Some(base_url) = resolve_update_base_url() else {
            return Ok(ManualUpdateCheckResult {
                status: "no_update".to_string(),
                version: None,
            });
        };
        let _guard = UpdateCheckGuard::acquire(&self.inner.in_progress)?;
        let arch = viewer_update_arch();
        let manifest_url = build_manifest_url(
            &base_url,
            arch,
            &self.inner.channel,
            self.inner.ring.as_deref(),
            env!("CARGO_PKG_VERSION"),
            &install_identity_seed(),
        );
        let if_none_match = self
            .inner
            .cached_etag
            .lock()
            .ok()
            .and_then(|guard| guard.clone());
        debug!(
            manifest_url = %manifest_url,
            if_none_match = ?if_none_match.as_deref(),
            automatic = matches!(apply_mode, UpdateApplyMode::Automatic),
            "viewer update: fetching manifest"
        );
        match fetch_manifest(&self.inner.client, &manifest_url, if_none_match.as_deref()).await? {
            ManifestFetchResult::NoUpdate | ManifestFetchResult::NotModified => {
                debug!("viewer update: manifest unchanged or no update");
                Ok(ManualUpdateCheckResult {
                    status: "no_update".to_string(),
                    version: None,
                })
            }
            ManifestFetchResult::Signed(signed) => {
                debug!(
                    manifest_version = %signed.manifest.version,
                    "viewer update: signed manifest received"
                );
                verify_manifest_signature(
                    EMBEDDED_MANIFEST_PUBLIC_KEY_DER,
                    &signed.manifest_bytes,
                    &signed.signature_b64,
                )?;
                let expected_manifest = UpdateManifestExpectation::for_artifact(
                    "viewer",
                    arch,
                    &self.inner.channel,
                    self.inner.ring.as_deref(),
                )?;
                validate_manifest_context(&signed.manifest, &expected_manifest)
                    .context("viewer update manifest context verification failed")?;
                if !is_update_newer(env!("CARGO_PKG_VERSION"), &signed.manifest.version)? {
                    debug!(
                        current_version = env!("CARGO_PKG_VERSION"),
                        manifest_version = %signed.manifest.version,
                        "viewer update: manifest not newer than running version"
                    );
                    return Ok(ManualUpdateCheckResult {
                        status: "no_update".to_string(),
                        version: None,
                    });
                }
                let package_url = build_package_url(
                    &base_url,
                    arch,
                    &self.inner.channel,
                    self.inner.ring.as_deref(),
                );
                let package_path = package_download_path(
                    "viewer",
                    &signed.manifest.version,
                    arch,
                    &signed.manifest.package.file_name,
                );
                debug!(
                    package_url = %package_url,
                    package_path = %package_path.display(),
                    "viewer update: downloading package"
                );
                download_file(
                    &self.inner.client,
                    &package_url,
                    &package_path,
                    signed.manifest.package.size_bytes,
                )
                .await?;
                let actual_size = fs::metadata(&package_path)
                    .with_context(|| format!("inspect {}", package_path.display()))?
                    .len();
                verify_package_size(signed.manifest.package.size_bytes, actual_size)
                    .context("viewer update package size verification failed")?;
                let actual_hash = sha256_hex_file(&package_path)?;
                verify_package_sha256(&signed.manifest.package.sha256, &actual_hash)
                    .context("viewer update package digest verification failed")?;
                if let Ok(mut guard) = self.inner.cached_etag.lock() {
                    *guard = signed.etag.clone();
                }
                if let Ok(mut guard) = self.inner.pending_package.lock() {
                    *guard = Some(PendingPackage {
                        package_path: package_path.clone(),
                        version: signed.manifest.version.clone(),
                    });
                }
                if matches!(apply_mode, UpdateApplyMode::Automatic) && viewer_is_idle(app) {
                    launch_pending_update(&package_path, Some(&signed.manifest.version), false)?;
                    request_app_exit(app.clone());
                    return Ok(ManualUpdateCheckResult {
                        status: "update_ready".to_string(),
                        version: Some(signed.manifest.version),
                    });
                }
                info!(
                    version = %signed.manifest.version,
                    "viewer update downloaded and staged until viewer is idle"
                );
                Ok(ManualUpdateCheckResult {
                    status: "update_ready".to_string(),
                    version: Some(signed.manifest.version),
                })
            }
        }
    }
}

fn log_embedded_manifest_key() {
    UPDATE_KEY_LOG_ONCE.call_once(|| {
        info!(
            manifest_public_key_sha256 = %sha256_hex_bytes(EMBEDDED_MANIFEST_PUBLIC_KEY_DER),
            manifest_public_key_bytes = EMBEDDED_MANIFEST_PUBLIC_KEY_DER.len(),
            "viewer updater trust key loaded"
        );
    });
}

struct UpdateCheckGuard<'a> {
    flag: &'a AtomicBool,
}

impl<'a> UpdateCheckGuard<'a> {
    fn acquire(flag: &'a AtomicBool) -> Result<Self> {
        flag.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .map_err(|_| anyhow!("viewer update check already in progress"))?;
        Ok(Self { flag })
    }
}

impl Drop for UpdateCheckGuard<'_> {
    fn drop(&mut self) {
        self.flag.store(false, Ordering::SeqCst);
    }
}

pub fn remember_update_api_base(api_base: &str) -> Result<()> {
    remember_update_api_base_at(api_base, &persisted_update_base_url_path())
}

fn remember_update_api_base_at(api_base: &str, path: &Path) -> Result<()> {
    let trimmed = api_base.trim();
    if trimmed.is_empty() {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    fs::write(path, trimmed).with_context(|| format!("write {}", path.display()))?;
    Ok(())
}

pub fn promote_pending_updater() -> Result<()> {
    #[cfg(not(windows))]
    {
        return Ok(());
    }
    #[cfg(windows)]
    {
        let install_dir = current_install_dir()?;
        let pending = install_dir.join(PENDING_UPDATER_FILE_NAME);
        if !pending.exists() {
            return Ok(());
        }
        let current = install_dir.join(UPDATER_FILE_NAME);
        let backup = install_dir.join("talos_viewer_updater.previous.exe");
        if current.exists() {
            let _ = fs::remove_file(&backup);
            fs::rename(&current, &backup)
                .with_context(|| format!("rename {} -> {}", current.display(), backup.display()))?;
        }
        fs::rename(&pending, &current)
            .with_context(|| format!("rename {} -> {}", pending.display(), current.display()))?;
        let _ = fs::remove_file(&backup);
        Ok(())
    }
}

pub fn take_update_notice() -> Result<Option<String>> {
    let notice_path = current_install_dir()?.join(UPDATE_NOTICE_FILE_NAME);
    if !notice_path.exists() {
        return Ok(None);
    }
    let version = fs::read_to_string(&notice_path)
        .with_context(|| format!("read {}", notice_path.display()))?;
    let _ = fs::remove_file(&notice_path);
    let trimmed = version.trim().to_string();
    if trimmed.is_empty() {
        return Ok(None);
    }
    Ok(Some(trimmed))
}

pub fn complete_update_exit_cleanup() {
    update_exit_notify().notify_waiters();
}

fn launch_pending_update(
    package_path: &Path,
    target_version: Option<&str>,
    show_completion_notice: bool,
) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        let _ = show_completion_notice;
        if let Some(version) = target_version {
            schedule_macos_relaunch_after_update(version)?;
        }
        Command::new("open")
            .arg(package_path)
            .spawn()
            .with_context(|| {
                format!(
                    "open macOS viewer update package {}",
                    package_path.display()
                )
            })?;
        debug!(
            package_path = %package_path.display(),
            "macOS viewer update package opened"
        );
        return Ok(());
    }

    #[cfg(not(target_os = "macos"))]
    {
        let install_dir = current_install_dir()?;
        let updater_path = install_dir.join(UPDATER_FILE_NAME);
        if !updater_path.exists() {
            return Err(anyhow!(
                "viewer updater executable not found at {}",
                updater_path.display()
            ));
        }
        let mut args = vec![
            "--package".to_string(),
            package_path.display().to_string(),
            "--install-dir".to_string(),
            install_dir.display().to_string(),
            "--wait-pid".to_string(),
            std::process::id().to_string(),
            "--relaunch".to_string(),
        ];
        if let Some(version) = target_version {
            args.push("--target-version".to_string());
            args.push(version.to_string());
        }
        if show_completion_notice {
            args.push("--show-completion-notice".to_string());
        }

        if can_write_install_dir(&install_dir) {
            match Command::new(&updater_path)
                .args(&args)
                .current_dir(&install_dir)
                .spawn()
            {
                Ok(_) => {}
                Err(err) if is_elevation_required(&err) => {
                    warn!(
                        error = %err,
                        updater_path = %updater_path.display(),
                        "viewer updater direct launch requested elevation; using packaged updater for per-user update"
                    );
                    launch_packaged_updater(package_path, &args, &install_dir)?;
                }
                Err(err) => {
                    return Err(err).with_context(|| format!("launch {}", updater_path.display()))
                }
            }
        } else {
            spawn_elevated(&updater_path, &args, &install_dir)?;
        }
        debug!(
            updater_path = %updater_path.display(),
            package_path = %package_path.display(),
            target_version = ?target_version,
            show_completion_notice,
            "viewer updater launched"
        );
        Ok(())
    }
}

#[cfg(target_os = "macos")]
fn schedule_macos_relaunch_after_update(target_version: &str) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let script_path = std::env::temp_dir().join(format!(
        "talos-viewer-update-relaunch-{}.sh",
        std::process::id()
    ));
    let script = format!(
        r#"#!/bin/sh
set -u

TARGET_VERSION='{target_version}'
APP_PATH='/Applications/Talos Viewer.app'
PLIST_PATH="$APP_PATH/Contents/Info.plist"
LOG_DIR="$HOME/Library/Logs/Talos"
LOG_PATH="$LOG_DIR/talos_viewer_update_relaunch.log"

mkdir -p "$LOG_DIR" >/dev/null 2>&1 || true
log() {{
  printf '%s %s\n' "$(date -u '+%Y-%m-%dT%H:%M:%SZ')" "$*" >> "$LOG_PATH" 2>/dev/null || true
}}

log "waiting for Talos Viewer $TARGET_VERSION to install"
attempt=1
while [ "$attempt" -le 1800 ]; do
  if [ -f "$PLIST_PATH" ]; then
    installed_version="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$PLIST_PATH" 2>/dev/null || printf '')"
    if [ "$installed_version" = "$TARGET_VERSION" ]; then
      if /usr/bin/open -na "$APP_PATH" >/dev/null 2>&1; then
        log "relaunched Talos Viewer $TARGET_VERSION"
        rm -f "$0"
        exit 0
      fi
      log "open failed for Talos Viewer $TARGET_VERSION"
    fi
  fi
  sleep 1
  attempt=$((attempt + 1))
done

log "timed out waiting for Talos Viewer $TARGET_VERSION to install"
rm -f "$0"
exit 0
"#,
        target_version = shell_single_quote_fragment(target_version)
    );
    fs::write(&script_path, script).with_context(|| format!("write {}", script_path.display()))?;
    fs::set_permissions(&script_path, fs::Permissions::from_mode(0o700))
        .with_context(|| format!("chmod {}", script_path.display()))?;
    Command::new("nohup")
        .arg("sh")
        .arg(&script_path)
        .current_dir("/")
        .spawn()
        .with_context(|| format!("spawn {}", script_path.display()))?;
    debug!(
        target_version,
        script_path = %script_path.display(),
        "scheduled macOS viewer relaunch watcher"
    );
    Ok(())
}

#[cfg(target_os = "macos")]
fn shell_single_quote_fragment(value: &str) -> String {
    value.replace('\'', r#"'\''"#)
}

fn request_app_exit(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let notified = update_exit_notify().notified();
        let _ = app.emit("viewer:update-before-exit", ());
        tokio::select! {
            _ = notified => {
                debug!("viewer update cleanup acknowledged");
            }
            _ = tokio::time::sleep(Duration::from_secs(12)) => {
                warn!("timed out waiting for viewer update cleanup acknowledgement");
            }
        }
        app.exit(0);
    });
}

fn update_exit_notify() -> &'static tokio::sync::Notify {
    UPDATE_EXIT_NOTIFY.get_or_init(tokio::sync::Notify::new)
}

#[cfg(windows)]
fn can_write_install_dir(install_dir: &Path) -> bool {
    let probe = install_dir.join(".viewer-updater-write-test");
    match OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&probe)
    {
        Ok(mut file) => {
            let _ = file.write_all(b"ok");
            let _ = fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

#[cfg(windows)]
fn is_elevation_required(error: &io::Error) -> bool {
    error.raw_os_error() == Some(WINDOWS_ERROR_ELEVATION_REQUIRED)
}

#[cfg(windows)]
fn launch_packaged_updater(package_path: &Path, args: &[String], install_dir: &Path) -> Result<()> {
    let updater_path = extract_packaged_updater(package_path)?;
    verify_authenticode(&updater_path)?;
    Command::new(&updater_path)
        .args(args)
        .current_dir(install_dir)
        .spawn()
        .with_context(|| format!("launch packaged viewer updater {}", updater_path.display()))?;
    debug!(
        updater_path = %updater_path.display(),
        package_path = %package_path.display(),
        "packaged viewer updater launched without elevation"
    );
    Ok(())
}

#[cfg(windows)]
fn extract_packaged_updater(package_path: &Path) -> Result<PathBuf> {
    let launcher_dir = update_root_dir().join("viewer").join("_launcher");
    if launcher_dir.exists() {
        let _ = fs::remove_dir_all(&launcher_dir);
    }
    fs::create_dir_all(&launcher_dir)
        .with_context(|| format!("create {}", launcher_dir.display()))?;
    let launcher_path = launcher_dir.join("talos_viewer_apply.exe");
    let file =
        File::open(package_path).with_context(|| format!("open {}", package_path.display()))?;
    let mut archive = ZipArchive::new(file).context("open viewer update package")?;
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .context("read viewer update package entry")?;
        if entry.is_dir() {
            continue;
        }
        let Some(file_name) = Path::new(entry.name())
            .file_name()
            .and_then(|value| value.to_str())
        else {
            continue;
        };
        if !file_name.eq_ignore_ascii_case(UPDATER_FILE_NAME) {
            continue;
        }
        let mut out_file = File::create(&launcher_path)
            .with_context(|| format!("create {}", launcher_path.display()))?;
        io::copy(&mut entry, &mut out_file)
            .with_context(|| format!("extract {}", launcher_path.display()))?;
        debug!(
            source_entry = entry.name(),
            launcher_path = %launcher_path.display(),
            "extracted packaged viewer updater"
        );
        return Ok(launcher_path);
    }
    Err(anyhow!(
        "viewer update package does not contain {}",
        UPDATER_FILE_NAME
    ))
}

#[cfg(windows)]
fn verify_authenticode(path: &Path) -> Result<()> {
    let script = format!(
        "(Get-AuthenticodeSignature -LiteralPath '{}').Status",
        ps_quote(&path.display().to_string())
    );
    let output = hidden_command("powershell.exe")
        .args(["-NoProfile", "-Command", &script])
        .output()
        .with_context(|| format!("verify Authenticode for {}", path.display()))?;
    let status = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if !output.status.success() || !status.eq_ignore_ascii_case("Valid") {
        return Err(anyhow!(
            "Authenticode verification failed for {} (status: {})",
            path.display(),
            if status.is_empty() {
                "unknown"
            } else {
                &status
            }
        ));
    }
    Ok(())
}

#[cfg(windows)]
fn hidden_command(program: &str) -> Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;

        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let mut command = Command::new(program);
        command.creation_flags(CREATE_NO_WINDOW);
        command
    }
    #[cfg(not(windows))]
    {
        Command::new(program)
    }
}

#[cfg(windows)]
fn spawn_elevated(executable: &Path, args: &[String], working_dir: &Path) -> Result<()> {
    let exe = ps_quote(&executable.display().to_string());
    let cwd = ps_quote(&working_dir.display().to_string());
    let arg_list = args
        .iter()
        .map(|arg| format!("'{}'", ps_quote(arg)))
        .collect::<Vec<_>>()
        .join(", ");
    let script = format!(
        "Start-Process -WindowStyle Hidden -FilePath '{exe}' -WorkingDirectory '{cwd}' -ArgumentList @({arg_list}) -Verb RunAs"
    );
    let status = Command::new("powershell.exe")
        .args(["-NoProfile", "-Command", &script])
        .status()
        .context("launch elevated viewer updater")?;
    if !status.success() {
        return Err(anyhow!("elevated viewer updater launch failed"));
    }
    Ok(())
}

#[cfg(windows)]
fn ps_quote(value: &str) -> String {
    value.replace('\'', "''")
}

fn viewer_is_idle(app: &AppHandle) -> bool {
    let windows = app.webview_windows();
    !windows
        .keys()
        .any(|label| label.starts_with(crate::SESSION_WINDOW_PREFIX))
}

fn resolve_update_base_url() -> Option<String> {
    let configured_update_base = env_string("RMM_VIEWER_UPDATE_BASE_URL");
    let api_backend_base = env_string("API_BACKEND_URL");
    let internal_api_base = env_string("INTERNAL_API_URL");
    let persisted_api_base = read_persisted_update_api_base(&persisted_update_base_url_path());
    resolve_update_base_url_from_values([
        configured_update_base.as_deref(),
        api_backend_base.as_deref(),
        internal_api_base.as_deref(),
        persisted_api_base.as_deref(),
    ])
}

fn env_string(name: &str) -> Option<String> {
    env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn read_persisted_update_api_base(path: &Path) -> Option<String> {
    fs::read_to_string(path)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn resolve_update_base_url_from_values(values: [Option<&str>; 4]) -> Option<String> {
    values
        .into_iter()
        .flatten()
        .find(|value| !value.trim().is_empty())
        .and_then(normalize_update_base_url)
}

fn persisted_update_base_url_path() -> PathBuf {
    if let Ok(base) = env::var("LOCALAPPDATA") {
        return PathBuf::from(base)
            .join("Talos")
            .join("Viewer")
            .join("update_api_base_url.txt");
    }
    env::temp_dir()
        .join("Talos")
        .join("Viewer")
        .join("update_api_base_url.txt")
}

fn current_install_dir() -> Result<PathBuf> {
    let exe = env::current_exe().context("resolve current exe")?;
    exe.parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| anyhow!("viewer install directory not found"))
}

fn install_identity_seed() -> String {
    current_install_dir()
        .ok()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| "viewer".to_string())
}

fn build_manifest_url(
    base_url: &str,
    arch: &str,
    channel: &str,
    ring: Option<&str>,
    current_version: &str,
    rollout_seed: &str,
) -> String {
    let mut url = format!(
        "{}/viewer/manifest?arch={arch}&channel={channel}&currentVersion={current_version}&rolloutSeed={rollout_seed}",
        base_url.trim_end_matches('/')
    );
    if let Some(ring) = ring {
        url.push_str("&ring=");
        url.push_str(ring);
    }
    url
}

fn build_package_url(base_url: &str, arch: &str, channel: &str, ring: Option<&str>) -> String {
    let mut url = format!(
        "{}/viewer/package?arch={}&channel={}",
        base_url.trim_end_matches('/'),
        urlencoding::encode(arch),
        urlencoding::encode(channel)
    );
    if let Some(ring) = ring {
        url.push_str("&ring=");
        url.push_str(&urlencoding::encode(ring));
    }
    url
}

fn package_download_path(product: &str, version: &str, arch: &str, file_name: &str) -> PathBuf {
    let ext = Path::new(file_name)
        .extension()
        .and_then(|value| value.to_str())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("zip");
    update_root_dir()
        .join(product)
        .join(format!("{product}-{arch}-{version}.{ext}"))
}

fn update_root_dir() -> PathBuf {
    if let Ok(base) = env::var("LOCALAPPDATA") {
        return PathBuf::from(base).join("Talos").join("updates");
    }
    env::temp_dir().join("Talos").join("updates")
}

fn viewer_update_arch() -> &'static str {
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    return "macos-arm64";
    #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
    return "macos-x64";
    #[cfg(not(target_os = "macos"))]
    return "x64";
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_TEST_DIRECTORY: AtomicU64 = AtomicU64::new(0);

    fn test_persisted_base_path() -> PathBuf {
        env::temp_dir()
            .join(format!(
                "talos-viewer-updater-test-{}-{}",
                std::process::id(),
                NEXT_TEST_DIRECTORY.fetch_add(1, Ordering::Relaxed)
            ))
            .join("update_api_base_url.txt")
    }

    #[test]
    fn package_download_path_uses_manifest_file_extension() {
        let path =
            package_download_path("viewer", "1.2.3", "macos-arm64", "Talos.Viewer.macos.pkg");
        assert_eq!(
            path.file_name().and_then(|value| value.to_str()),
            Some("viewer-macos-arm64-1.2.3.pkg")
        );
    }

    #[test]
    fn build_urls_include_selected_viewer_arch() {
        let base = "https://api.example.test/rmm/updates/";
        assert!(
            build_manifest_url(base, "macos-x64", "stable", None, "1.0.0", "seed")
                .contains("arch=macos-x64")
        );
        assert_eq!(
            build_package_url(base, "macos-arm64", "stable", Some("pilot")),
            "https://api.example.test/rmm/updates/viewer/package?arch=macos-arm64&channel=stable&ring=pilot"
        );
    }

    #[test]
    fn update_endpoint_is_disabled_without_configuration() {
        assert_eq!(resolve_update_base_url_from_values([None; 4]), None);
    }

    #[test]
    fn persisted_session_api_base_enables_self_hosted_updates() {
        let path = test_persisted_base_path();
        remember_update_api_base_at(" https://community.example.test/api/ ", &path).unwrap();

        let persisted = read_persisted_update_api_base(&path);
        let resolved =
            resolve_update_base_url_from_values([None, None, None, persisted.as_deref()]);

        assert_eq!(
            resolved,
            Some("https://community.example.test/api/rmm/updates".to_string())
        );
        if let Some(parent) = path.parent() {
            fs::remove_dir_all(parent).unwrap();
        }
    }

    #[test]
    fn explicit_update_configuration_precedes_persisted_session_base() {
        assert_eq!(
            resolve_update_base_url_from_values([
                Some("https://configured.example.test/rmm/updates"),
                None,
                None,
                Some("https://persisted.example.test"),
            ]),
            Some("https://configured.example.test/rmm/updates".to_string())
        );
    }

    #[test]
    fn invalid_explicit_endpoint_does_not_fall_through_to_persisted_state() {
        assert_eq!(
            resolve_update_base_url_from_values([
                Some("file:///tmp/updates"),
                None,
                None,
                Some("https://persisted.example.test"),
            ]),
            None
        );
    }
}
