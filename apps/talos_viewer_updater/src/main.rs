#![cfg_attr(windows, windows_subsystem = "windows")]

use std::{
    env,
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    process::Command,
    sync::OnceLock,
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{anyhow, bail, Context, Result};
use tracing::{debug, error, info, warn};
use walkdir::WalkDir;
use zip::ZipArchive;

const SELF_FILE_NAME: &str = "talos_viewer_updater.exe";
const PENDING_SELF_FILE_NAME: &str = "talos_viewer_updater.next.exe";
const UPDATE_NOTICE_FILE_NAME: &str = ".viewer_update_notice";
static UPDATE_LOG_PATH: OnceLock<PathBuf> = OnceLock::new();

/// Clear `RUST_LOG`; file log level uses `talos_protocol::rmm_tracing_filter_directive`.
fn strip_legacy_log_env_vars() {
    env::remove_var("RUST_LOG");
}

fn main() -> Result<()> {
    strip_legacy_log_env_vars();
    write_bootstrap_log("main_start", None);
    if let Err(err) = init_file_logging() {
        write_bootstrap_log("init_file_logging_err", Some(&err.to_string()));
        return Err(anyhow!(
            "failed to initialize viewer updater file logging: {err}"
        ));
    }
    write_bootstrap_log("init_file_logging_ok", None);

    let args = Args::parse()?;
    info!("starting viewer update");
    debug!(
        package_path = %args.package_path.display(),
        install_dir = %args.install_dir.display(),
        relaunch = args.relaunch,
        wait_pid = ?args.wait_pid,
        target_version = ?args.target_version,
        show_completion_notice = args.show_completion_notice,
        "viewer update args"
    );
    wait_for_parent_exit(args.wait_pid)?;
    if let Err(err) = apply_viewer_update(&args) {
        error!(error = %err, "viewer update failed");
        return Err(err);
    }
    if args.show_completion_notice {
        if let Some(version) = args.target_version.as_deref() {
            if let Err(err) = write_update_notice(&args.install_dir, version) {
                warn!(error = %err, version, "failed to write viewer update notice");
            }
        }
    }
    if args.relaunch {
        relaunch_viewer(&args.install_dir)?;
    }
    info!("viewer update completed successfully");
    Ok(())
}

struct Args {
    package_path: PathBuf,
    install_dir: PathBuf,
    wait_pid: Option<u32>,
    relaunch: bool,
    target_version: Option<String>,
    show_completion_notice: bool,
}

impl Args {
    fn parse() -> Result<Self> {
        let mut package_path = None;
        let mut install_dir = None;
        let mut wait_pid = None;
        let mut relaunch = false;
        let mut target_version = None;
        let mut show_completion_notice = false;
        let mut iter = env::args().skip(1);
        while let Some(arg) = iter.next() {
            match arg.as_str() {
                "--package" => package_path = iter.next().map(PathBuf::from),
                "--install-dir" => install_dir = iter.next().map(PathBuf::from),
                "--wait-pid" => {
                    wait_pid = iter
                        .next()
                        .map(|value| value.parse::<u32>().context("parse wait pid"))
                        .transpose()?;
                }
                "--relaunch" => relaunch = true,
                "--target-version" => target_version = iter.next(),
                "--show-completion-notice" => show_completion_notice = true,
                other => bail!("unknown arg: {other}"),
            }
        }
        Ok(Self {
            package_path: package_path.ok_or_else(|| anyhow!("--package is required"))?,
            install_dir: install_dir.ok_or_else(|| anyhow!("--install-dir is required"))?,
            wait_pid,
            relaunch,
            target_version: target_version
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty()),
            show_completion_notice,
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
fn windows_log_path_candidates() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Ok(base) = env::var("PROGRAMDATA") {
        paths.push(
            PathBuf::from(base)
                .join("Talos")
                .join("logs")
                .join("talos_viewer_update.log"),
        );
    }
    paths.push(PathBuf::from(
        r"C:\ProgramData\Talos\logs\talos_viewer_update.log",
    ));
    paths.push(env::temp_dir().join("talos_viewer_update.log"));
    paths.push(PathBuf::from(r"C:\Windows\Temp\talos_viewer_update.log"));
    paths
}

#[cfg(not(target_os = "windows"))]
fn windows_log_path_candidates() -> Vec<PathBuf> {
    vec![env::temp_dir().join("talos_viewer_update.log")]
}

fn resolve_log_path() -> PathBuf {
    for template in windows_log_path_candidates() {
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
    windows_log_path_candidates()
        .into_iter()
        .next()
        .unwrap_or_else(|| env::temp_dir().join("talos_viewer_update.log"))
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

fn apply_viewer_update(args: &Args) -> Result<()> {
    let staging_dir = args.install_dir.join("_viewer_update_staging");
    if staging_dir.exists() {
        debug!(path = %staging_dir.display(), "removing previous viewer update staging directory");
        let _ = fs::remove_dir_all(&staging_dir);
    }
    fs::create_dir_all(&staging_dir)
        .with_context(|| format!("create {}", staging_dir.display()))?;
    info!(path = %staging_dir.display(), "created viewer update staging directory");
    extract_zip(&args.package_path, &staging_dir)?;
    replace_directory_contents(&staging_dir, &args.install_dir)?;
    if let Some(version) = args.target_version.as_deref() {
        if let Err(err) = update_arp_display_version(version) {
            warn!(error = %err, version, "failed to update viewer ARP display version");
        }
    }
    if let Err(err) = fs::remove_dir_all(&staging_dir) {
        warn!(error = %err, path = %staging_dir.display(), "failed to remove viewer staging directory after update");
    } else {
        debug!(path = %staging_dir.display(), "removed viewer update staging directory");
    }
    Ok(())
}

fn extract_zip(zip_path: &Path, destination: &Path) -> Result<()> {
    let t0 = Instant::now();
    debug!(
        package_path = %zip_path.display(),
        destination = %destination.display(),
        "extracting viewer update package"
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
        debug!(path = %out_path.display(), index, "extracted viewer update zip entry");
    }
    debug!(
        destination = %destination.display(),
        entry_count,
        elapsed_ms = t0.elapsed().as_millis() as u64,
        "finished extracting viewer update package"
    );
    Ok(())
}

fn replace_directory_contents(staging_dir: &Path, install_dir: &Path) -> Result<()> {
    let t0 = Instant::now();
    debug!(
        staging_dir = %staging_dir.display(),
        install_dir = %install_dir.display(),
        "replacing installed viewer files from staging"
    );
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
        let file_name = relative
            .file_name()
            .and_then(|value| value.to_str())
            .ok_or_else(|| anyhow!("invalid staging file name"))?;
        let destination = install_dir.join(relative);
        if file_name.eq_ignore_ascii_case(SELF_FILE_NAME) {
            verify_authenticode(entry.path())?;
            let pending = install_dir.join(PENDING_SELF_FILE_NAME);
            debug!(
                source = %entry.path().display(),
                destination = %pending.display(),
                "staging pending viewer updater self-update"
            );
            fs::copy(entry.path(), &pending)
                .with_context(|| format!("copy {}", pending.display()))?;
            continue;
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
                "creating temporary viewer backup before file replacement"
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
            "copying viewer update file into install directory"
        );
        fs::copy(entry.path(), &destination)
            .with_context(|| format!("copy {}", destination.display()))?;
        let _ = fs::remove_file(&backup_path);
        files_replaced += 1;
    }
    info!(
        files_replaced,
        elapsed_ms = t0.elapsed().as_millis() as u64,
        "replaced viewer files successfully"
    );
    Ok(())
}

fn verify_authenticode(path: &Path) -> Result<()> {
    let script = format!(
        "(Get-AuthenticodeSignature -LiteralPath '{}').Status",
        path.display().to_string().replace('\'', "''")
    );
    let output = hidden_command("powershell.exe")
        .args(["-NoProfile", "-Command", &script])
        .output()
        .with_context(|| format!("verify Authenticode for {}", path.display()))?;
    let status = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if !output.status.success() || !status.eq_ignore_ascii_case("Valid") {
        bail!(
            "Authenticode verification failed for {} (status: {})",
            path.display(),
            if status.is_empty() {
                "unknown"
            } else {
                &status
            }
        );
    }
    debug!(path = %path.display(), "viewer Authenticode verification succeeded");
    Ok(())
}

fn relaunch_viewer(install_dir: &Path) -> Result<()> {
    let viewer_path = install_dir.join("talos_viewer.exe");
    debug!(path = %viewer_path.display(), "relaunching viewer after update");
    if !viewer_path.exists() {
        bail!("viewer executable not found at {}", viewer_path.display());
    }

    #[cfg(windows)]
    {
        match hidden_command("explorer.exe").arg(&viewer_path).spawn() {
            Ok(_) => {
                info!(path = %viewer_path.display(), "viewer relaunch requested via Explorer");
                return Ok(());
            }
            Err(err) => {
                warn!(
                    error = %err,
                    path = %viewer_path.display(),
                    "failed to relaunch viewer via Explorer; falling back to direct spawn"
                );
            }
        }
    }

    Command::new(&viewer_path)
        .current_dir(install_dir)
        .spawn()
        .with_context(|| format!("spawn {}", viewer_path.display()))?;
    info!(path = %viewer_path.display(), "viewer relaunched directly");
    Ok(())
}

fn write_update_notice(install_dir: &Path, version: &str) -> Result<()> {
    let notice_path = install_dir.join(UPDATE_NOTICE_FILE_NAME);
    fs::write(&notice_path, format!("{}\n", version.trim()))
        .with_context(|| format!("write {}", notice_path.display()))?;
    info!(path = %notice_path.display(), version, "wrote viewer update notice");
    Ok(())
}

fn update_arp_display_version(version: &str) -> Result<()> {
    const DISPLAY_NAME: &str = "Talos Viewer Installer";
    let version = version.trim();
    if version.is_empty() {
        return Ok(());
    }

    let script = format!(
        r#"$paths = @(
  'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\*',
  'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\*',
  'HKLM:\SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall\*'
)
$entries = foreach ($path in $paths) {{
  Get-ItemProperty -Path $path -ErrorAction SilentlyContinue |
    Where-Object {{ $_.DisplayName -eq '{display_name}' }}
}}
if (-not $entries) {{
  throw 'ARP entry not found for {display_name}'
}}
foreach ($entry in $entries) {{
  Set-ItemProperty -Path $entry.PSPath -Name DisplayVersion -Value '{version}'
}}
"#,
        display_name = ps_quote(DISPLAY_NAME),
        version = ps_quote(version),
    );
    let output = hidden_command("powershell.exe")
        .args(["-NoProfile", "-Command", &script])
        .output()
        .context("update viewer ARP display version")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        bail!(
            "failed to update viewer ARP display version to {} ({})",
            version,
            if stderr.is_empty() {
                "unknown error"
            } else {
                &stderr
            }
        );
    }
    debug!(
        version,
        display_name = DISPLAY_NAME,
        "updated viewer ARP display version"
    );
    Ok(())
}

fn ps_quote(value: &str) -> String {
    value.replace('\'', "''")
}

#[cfg(windows)]
fn hidden_command(program: &str) -> Command {
    use std::os::windows::process::CommandExt;

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let mut command = Command::new(program);
    command.creation_flags(CREATE_NO_WINDOW);
    command
}

#[cfg(not(windows))]
fn hidden_command(program: &str) -> Command {
    Command::new(program)
}

fn wait_for_parent_exit(pid: Option<u32>) -> Result<()> {
    #[cfg(windows)]
    {
        use windows_sys::Win32::{
            Foundation::CloseHandle,
            System::Threading::{
                OpenProcess, WaitForSingleObject, PROCESS_QUERY_LIMITED_INFORMATION,
            },
        };
        const SYNCHRONIZE_ACCESS: u32 = 0x0010_0000;
        const WAIT_OBJECT_0: u32 = 0;

        let Some(pid) = pid else {
            debug!("wait_for_parent_exit: no pid, skipping");
            return Ok(());
        };
        debug!(pid, "wait_for_parent_exit: waiting on parent process");
        unsafe {
            let handle = OpenProcess(
                PROCESS_QUERY_LIMITED_INFORMATION | SYNCHRONIZE_ACCESS,
                0,
                pid,
            );
            if handle.is_null() {
                return Ok(());
            }
            let wait = WaitForSingleObject(handle, 30_000);
            CloseHandle(handle);
            if wait != WAIT_OBJECT_0 {
                thread::sleep(Duration::from_secs(2));
            }
        }
        return Ok(());
    }

    #[allow(unreachable_code)]
    {
        let _ = pid;
        thread::sleep(Duration::from_secs(2));
        Ok(())
    }
}
