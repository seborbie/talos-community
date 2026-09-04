use std::{
    env,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    os::unix::io::AsRawFd,
    path::Path,
    process::{Command, Stdio},
    thread,
    time::Duration,
};

use anyhow::{anyhow, bail, Context, Result};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

const STATE_PATH: &str = "/var/lib/talos/shell-credential.json";
const STATE_DIR: &str = "/var/lib/talos";
const STATE_LOCK_PATH: &str = "/var/lib/talos/shell-credential.lock";
const DEFAULT_USER: &str = "talos";
const SUDOERS_DIR: &str = "/etc/sudoers.d";
const SUDOERS_FILE: &str = "/etc/sudoers.d/talos-managed-shell";
const ACCOUNT_COMMAND_ATTEMPTS: usize = 12;
const ACCOUNT_LOCK_WAIT_ATTEMPTS: usize = 20;
const STALE_ACCOUNT_LOCK_SECS: u64 = 8;
const ACCOUNT_DATABASE_LOCK_PATHS: &[&str] = &[
    "/etc/passwd.lock",
    "/etc/group.lock",
    "/etc/shadow.lock",
    "/etc/gshadow.lock",
    "/etc/.pwd.lock",
];

#[derive(Debug, Clone)]
pub struct PendingLinuxShellCredential {
    pub username: String,
    pub password: String,
    pub credential_id: String,
    pub version: i32,
    pub generated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CredentialState {
    username: String,
    credential_id: String,
    version: i32,
    generated_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pending_password: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    reported_at: Option<String>,
}

pub fn resolve_managed_shell_username() -> Option<String> {
    read_state()
        .ok()
        .flatten()
        .map(|state| state.username)
        .or_else(|| {
            env::var("RMM_SHELL_USER")
                .ok()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
        })
}

pub fn ensure_managed_shell_credential() -> Result<Option<PendingLinuxShellCredential>> {
    let _lock = acquire_state_lock()?;
    let Some(mut state) = read_state()? else {
        let username = choose_username()?;
        let password = generate_password()?;
        ensure_user(&username, &password)?;
        ensure_sudoers(&username)?;
        let state = CredentialState {
            username: username.clone(),
            credential_id: Uuid::new_v4().to_string(),
            version: 1,
            generated_at: Utc::now().to_rfc3339(),
            pending_password: Some(password.clone()),
            reported_at: None,
        };
        write_state(&state)?;
        return Ok(Some(PendingLinuxShellCredential {
            username,
            password,
            credential_id: state.credential_id,
            version: state.version,
            generated_at: state.generated_at,
        }));
    };

    ensure_user_exists_and_sudoers(&state.username)?;
    if let Some(password) = state.pending_password.clone() {
        return Ok(Some(PendingLinuxShellCredential {
            username: state.username,
            password,
            credential_id: state.credential_id,
            version: state.version,
            generated_at: state.generated_at,
        }));
    }
    state
        .reported_at
        .get_or_insert_with(|| Utc::now().to_rfc3339());
    write_state(&state)?;
    Ok(None)
}

pub fn mark_shell_credential_reported(credential_id: &str) -> Result<()> {
    let Some(mut state) = read_state()? else {
        return Ok(());
    };
    if state.credential_id != credential_id {
        return Ok(());
    }
    state.pending_password = None;
    state.reported_at = Some(Utc::now().to_rfc3339());
    write_state(&state)
}

fn read_state() -> Result<Option<CredentialState>> {
    let path = Path::new(STATE_PATH);
    if !path.is_file() {
        return Ok(None);
    }
    let data = fs::read_to_string(path).with_context(|| format!("read {STATE_PATH}"))?;
    let state = serde_json::from_str(&data).with_context(|| format!("parse {STATE_PATH}"))?;
    Ok(Some(state))
}

fn write_state(state: &CredentialState) -> Result<()> {
    fs::create_dir_all(STATE_DIR).with_context(|| format!("create {STATE_DIR}"))?;
    fs::set_permissions(STATE_DIR, fs::Permissions::from_mode(0o700)).ok();
    let tmp_path = format!("{STATE_PATH}.tmp");
    let data = serde_json::to_vec_pretty(state).context("serialize shell credential state")?;
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .mode(0o600)
        .open(&tmp_path)
        .with_context(|| format!("open {tmp_path}"))?;
    file.write_all(&data)
        .with_context(|| format!("write {tmp_path}"))?;
    file.sync_all().ok();
    fs::set_permissions(&tmp_path, fs::Permissions::from_mode(0o600)).ok();
    fs::rename(&tmp_path, STATE_PATH).with_context(|| format!("replace {STATE_PATH}"))?;
    Ok(())
}

fn choose_username() -> Result<String> {
    let desired = env::var("RMM_SHELL_USER")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| DEFAULT_USER.to_string());
    if user_uid(&desired)? == Some(0) {
        bail!("refusing to use root-like {desired} user for Talos shell");
    }
    Ok(desired)
}

fn ensure_user_exists_and_sudoers(username: &str) -> Result<()> {
    if !user_exists(username)? {
        bail!("managed Talos shell user {username} no longer exists");
    }
    ensure_sudoers(username)
}

fn ensure_user(username: &str, password: &str) -> Result<()> {
    let shell = preferred_shell();
    if !user_exists(username)? {
        let home_dir = format!("{STATE_DIR}/shell-users/{username}");
        create_user_with_retry(username, &home_dir, &shell)
            .with_context(|| format!("create managed Talos shell user {username}"))?;
    }
    set_password(username, password)?;
    let mut usermod = Command::new("usermod");
    usermod.args(["--shell", &shell, username]);
    let _ = run_account_command(&mut usermod, "update managed Talos shell user shell");
    Ok(())
}

fn create_user_with_retry(username: &str, home_dir: &str, shell: &str) -> Result<()> {
    ensure_group(username)?;
    if user_exists(username)? {
        return Ok(());
    }

    let mut useradd = Command::new("useradd");
    useradd.args([
        "--system",
        "--create-home",
        "--home-dir",
        home_dir,
        "--shell",
        shell,
        "--gid",
        username,
        username,
    ]);
    if run_account_command(&mut useradd, "create managed Talos shell user with useradd").is_ok()
        || user_exists(username)?
    {
        return Ok(());
    }
    bail!("managed Talos shell user {username} was not created");
}

fn ensure_group(groupname: &str) -> Result<()> {
    if group_exists(groupname)? {
        return Ok(());
    }
    let mut groupadd = Command::new("groupadd");
    groupadd.args(["--system", groupname]);
    if run_account_command(&mut groupadd, "create managed Talos shell group").is_ok()
        || group_exists(groupname)?
    {
        return Ok(());
    }
    bail!("managed Talos shell group {groupname} was not created");
}

fn set_password(username: &str, password: &str) -> Result<()> {
    let mut last_error: Option<anyhow::Error> = None;
    for attempt in 1..=ACCOUNT_COMMAND_ATTEMPTS {
        wait_for_account_database_locks()?;
        match run_chpasswd(username, password) {
            Ok(()) => {
                let _ = run_account_command(
                    Command::new("passwd").args(["-u", username]),
                    "unlock managed Talos shell user",
                );
                return Ok(());
            }
            Err(error) => {
                last_error = Some(error);
                let _ = wait_for_account_database_locks();
                if attempt < ACCOUNT_COMMAND_ATTEMPTS {
                    thread::sleep(Duration::from_millis(250 * attempt as u64));
                }
            }
        }
    }
    Err(last_error.unwrap_or_else(|| anyhow!("chpasswd failed")))
        .context("set managed Talos shell user password")
}

fn run_chpasswd(username: &str, password: &str) -> Result<()> {
    let mut child = Command::new("chpasswd")
        .stdin(Stdio::piped())
        .spawn()
        .context("spawn chpasswd")?;
    {
        let stdin = child.stdin.as_mut().context("open chpasswd stdin")?;
        writeln!(stdin, "{username}:{password}").context("write chpasswd input")?;
    }
    let status = child.wait().context("wait for chpasswd")?;
    if status.success() {
        Ok(())
    } else {
        Err(anyhow!("chpasswd failed with {status}"))
    }
}

fn ensure_sudoers(username: &str) -> Result<()> {
    fs::create_dir_all(SUDOERS_DIR).with_context(|| format!("create {SUDOERS_DIR}"))?;
    let line = format!("{username} ALL=(ALL:ALL) ALL\n");
    let tmp_path = format!("{SUDOERS_FILE}.tmp");
    fs::write(&tmp_path, line).with_context(|| format!("write {tmp_path}"))?;
    fs::set_permissions(&tmp_path, fs::Permissions::from_mode(0o440))
        .with_context(|| format!("chmod {tmp_path}"))?;
    if command_exists("visudo") {
        run(Command::new("visudo").args(["-cf", &tmp_path]))
            .with_context(|| format!("validate {tmp_path}"))?;
    }
    fs::rename(&tmp_path, SUDOERS_FILE).with_context(|| format!("replace {SUDOERS_FILE}"))?;
    fs::set_permissions(SUDOERS_FILE, fs::Permissions::from_mode(0o440)).ok();
    Ok(())
}

fn preferred_shell() -> String {
    if Path::new("/bin/bash").is_file() {
        "/bin/bash".to_string()
    } else {
        "/bin/sh".to_string()
    }
}

fn generate_password() -> Result<String> {
    let mut bytes = [0u8; 32];
    File::open("/dev/urandom")
        .context("open /dev/urandom")?
        .read_exact(&mut bytes)
        .context("read random password bytes")?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

fn acquire_state_lock() -> Result<File> {
    fs::create_dir_all(STATE_DIR).with_context(|| format!("create {STATE_DIR}"))?;
    fs::set_permissions(STATE_DIR, fs::Permissions::from_mode(0o700)).ok();
    let file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .mode(0o600)
        .open(STATE_LOCK_PATH)
        .with_context(|| format!("open {STATE_LOCK_PATH}"))?;
    let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) };
    if rc != 0 {
        return Err(std::io::Error::last_os_error())
            .with_context(|| format!("lock {STATE_LOCK_PATH}"));
    }
    Ok(file)
}

fn wait_for_account_database_locks() -> Result<()> {
    for attempt in 1..=ACCOUNT_LOCK_WAIT_ATTEMPTS {
        let mut blocked_paths = Vec::new();
        for &lock_path in ACCOUNT_DATABASE_LOCK_PATHS {
            if account_lock_blocks(Path::new(lock_path))? {
                blocked_paths.push(lock_path);
            }
        }
        if blocked_paths.is_empty() {
            return Ok(());
        }
        if attempt < ACCOUNT_LOCK_WAIT_ATTEMPTS {
            thread::sleep(Duration::from_millis(250 * attempt.min(8) as u64));
        }
    }

    let remaining = ACCOUNT_DATABASE_LOCK_PATHS
        .iter()
        .filter(|&&lock_path| Path::new(lock_path).is_file())
        .copied()
        .collect::<Vec<_>>();
    if remaining.is_empty() {
        Ok(())
    } else {
        bail!(
            "account database remains locked by {}",
            remaining.join(", ")
        )
    }
}

fn account_lock_blocks(path: &Path) -> Result<bool> {
    let metadata = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error).with_context(|| format!("metadata {}", path.display())),
    };

    if file_has_open_fd(metadata.dev(), metadata.ino())? {
        return Ok(true);
    }

    let stale = metadata
        .modified()
        .ok()
        .and_then(|modified| modified.elapsed().ok())
        .map(|age| age >= Duration::from_secs(STALE_ACCOUNT_LOCK_SECS))
        .unwrap_or(false);
    if !stale {
        return Ok(true);
    }

    match fs::remove_file(path) {
        Ok(()) => Ok(false),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error).with_context(|| format!("remove stale {}", path.display())),
    }
}

fn file_has_open_fd(target_dev: u64, target_ino: u64) -> Result<bool> {
    let entries = match fs::read_dir("/proc") {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error).context("read /proc"),
    };

    for entry in entries.flatten() {
        let name = entry.file_name();
        if !name
            .to_string_lossy()
            .bytes()
            .all(|byte| byte.is_ascii_digit())
        {
            continue;
        }
        let Ok(fds) = fs::read_dir(entry.path().join("fd")) else {
            continue;
        };
        for fd in fds.flatten() {
            let Ok(metadata) = fs::metadata(fd.path()) else {
                continue;
            };
            if metadata.dev() == target_dev && metadata.ino() == target_ino {
                return Ok(true);
            }
        }
    }

    Ok(false)
}

fn user_exists(username: &str) -> Result<bool> {
    Ok(Command::new("id")
        .arg("-u")
        .arg(username)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()?
        .success())
}

fn group_exists(groupname: &str) -> Result<bool> {
    Ok(Command::new("getent")
        .arg("group")
        .arg(groupname)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()?
        .success())
}

fn user_uid(username: &str) -> Result<Option<u32>> {
    let output = Command::new("id")
        .arg("-u")
        .arg(username)
        .stderr(Stdio::null())
        .output()?;
    if !output.status.success() {
        return Ok(None);
    }
    let text = String::from_utf8_lossy(&output.stdout);
    Ok(text.trim().parse::<u32>().ok())
}

fn command_exists(name: &str) -> bool {
    Command::new("sh")
        .args(["-c", &format!("command -v {name} >/dev/null 2>&1")])
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

fn run(command: &mut Command) -> Result<()> {
    let status = command.status().context("run command")?;
    if status.success() {
        Ok(())
    } else {
        Err(anyhow!("command failed with {status}"))
    }
}

fn run_account_command(command: &mut Command, description: &str) -> Result<()> {
    let program = command.get_program().to_string_lossy().into_owned();
    let args = command
        .get_args()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join(" ");
    let command_text = if args.is_empty() {
        program
    } else {
        format!("{program} {args}")
    };
    let mut last_error: Option<anyhow::Error> = None;
    for attempt in 1..=ACCOUNT_COMMAND_ATTEMPTS {
        wait_for_account_database_locks()?;
        match run(command) {
            Ok(()) => return Ok(()),
            Err(error) => {
                last_error = Some(error);
                let _ = wait_for_account_database_locks();
                if attempt < ACCOUNT_COMMAND_ATTEMPTS {
                    thread::sleep(Duration::from_millis(250 * attempt as u64));
                }
            }
        }
    }
    Err(last_error.unwrap_or_else(|| anyhow!("command failed")))
        .with_context(|| format!("{description}: {command_text}"))
}
