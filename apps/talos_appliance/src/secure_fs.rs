use std::{
    ffi::OsString,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
};

use anyhow::{bail, Context, Result};
use rand::{rngs::OsRng, RngCore};
use serde::{de::DeserializeOwned, Serialize};

#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

pub const MAX_STATE_FILE_BYTES: usize = 4 * 1024 * 1024;

#[cfg(any(windows, test))]
const WINDOWS_PROTECTED_SDDL: &str = "D:P(A;OICI;FA;;;OW)(A;OICI;FA;;;BA)(A;OICI;FA;;;SY)";

pub fn default_state_root() -> Result<PathBuf> {
    #[cfg(windows)]
    {
        let program_data = std::env::var_os("ProgramData")
            .context("ProgramData is not set; pass --state-dir explicitly")?;
        return Ok(PathBuf::from(program_data).join("Talos").join("Server"));
    }
    #[cfg(not(windows))]
    {
        Ok(PathBuf::from("/var/lib/talos-server"))
    }
}

pub fn ensure_secure_directory(path: &Path) -> Result<()> {
    if !path.is_absolute() {
        bail!("protected directory must be absolute");
    }
    reject_symlinks_in_existing_path(path)?;
    fs::create_dir_all(path)
        .with_context(|| format!("could not create protected directory {}", path.display()))?;
    reject_symlinks_in_existing_path(path)?;
    let metadata = fs::metadata(path)
        .with_context(|| format!("could not inspect protected directory {}", path.display()))?;
    if !metadata.is_dir() {
        bail!("protected path {} is not a directory", path.display());
    }
    #[cfg(unix)]
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).with_context(|| {
        format!(
            "could not set restrictive permissions on directory {}",
            path.display()
        )
    })?;
    #[cfg(windows)]
    windows_acl::apply_owner_admin_system_acl(path)?;
    Ok(())
}

pub fn read_protected_file(path: &Path, max_bytes: usize) -> Result<Vec<u8>> {
    reject_symlinks_in_existing_path(path)?;
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("could not inspect protected file {}", path.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        bail!("protected path {} must be a regular file", path.display());
    }
    if metadata.len() > max_bytes as u64 {
        bail!("protected file {} exceeds its size limit", path.display());
    }
    #[cfg(windows)]
    windows_acl::apply_owner_admin_system_acl(path)?;
    #[cfg(unix)]
    if metadata.permissions().mode() & 0o077 != 0 {
        bail!(
            "protected file {} must not be accessible by group or other users",
            path.display()
        );
    }
    let file = File::open(path)
        .with_context(|| format!("could not open protected file {}", path.display()))?;
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take((max_bytes + 1) as u64)
        .read_to_end(&mut bytes)
        .with_context(|| format!("could not read protected file {}", path.display()))?;
    if bytes.len() > max_bytes {
        bail!("protected file {} exceeds its size limit", path.display());
    }
    Ok(bytes)
}

pub fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T> {
    let bytes = read_protected_file(path, MAX_STATE_FILE_BYTES)?;
    serde_json::from_slice(&bytes)
        .with_context(|| format!("{} contains malformed JSON", path.display()))
}

pub fn atomic_write_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let mut bytes =
        serde_json::to_vec_pretty(value).context("could not serialize protected JSON")?;
    bytes.push(b'\n');
    atomic_write(path, &bytes)
}

pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .context("protected file path has no parent directory")?;
    ensure_secure_directory(parent)?;
    if let Ok(metadata) = fs::symlink_metadata(path) {
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            bail!(
                "refusing to replace non-regular protected path {}",
                path.display()
            );
        }
    }

    let temporary_path = temporary_sibling(path)?;
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let write_result = (|| -> Result<()> {
        let mut file = options.open(&temporary_path).with_context(|| {
            format!(
                "could not create temporary protected file {}",
                temporary_path.display()
            )
        })?;
        #[cfg(windows)]
        windows_acl::apply_owner_admin_system_acl(&temporary_path)?;
        file.write_all(bytes)
            .context("could not write temporary protected file")?;
        file.sync_all()
            .context("could not flush temporary protected file")?;
        #[cfg(unix)]
        file.set_permissions(fs::Permissions::from_mode(0o600))
            .context("could not set protected file permissions")?;
        drop(file);
        fs::rename(&temporary_path, path)
            .with_context(|| format!("could not atomically replace {}", path.display()))?;
        #[cfg(windows)]
        windows_acl::apply_owner_admin_system_acl(path)?;
        sync_directory(parent)?;
        Ok(())
    })();
    if write_result.is_err() {
        let _ = fs::remove_file(&temporary_path);
    }
    write_result
}

pub fn copy_protected_file(source: &Path, destination: &Path, max_bytes: usize) -> Result<()> {
    let bytes = read_protected_file(source, max_bytes)?;
    atomic_write(destination, &bytes)
}

pub fn harden_regular_file(path: &Path) -> Result<()> {
    reject_symlinks_in_existing_path(path)?;
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("could not inspect protected file {}", path.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        bail!("protected path {} must be a regular file", path.display());
    }
    #[cfg(unix)]
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).with_context(|| {
        format!(
            "could not set restrictive permissions on file {}",
            path.display()
        )
    })?;
    #[cfg(windows)]
    windows_acl::apply_owner_admin_system_acl(path)?;
    Ok(())
}

pub fn copy_large_protected_file(source: &Path, destination: &Path) -> Result<()> {
    reject_symlinks_in_existing_path(source)?;
    let metadata = fs::symlink_metadata(source)
        .with_context(|| format!("could not inspect protected source {}", source.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        bail!(
            "protected source {} must be a regular file",
            source.display()
        );
    }
    #[cfg(unix)]
    if metadata.permissions().mode() & 0o077 != 0 {
        bail!(
            "protected source {} must not be accessible by group or other users",
            source.display()
        );
    }
    #[cfg(windows)]
    windows_acl::apply_owner_admin_system_acl(source)?;
    if destination.exists() {
        bail!(
            "refusing to overwrite protected destination {}",
            destination.display()
        );
    }
    let parent = destination
        .parent()
        .context("protected destination has no parent")?;
    ensure_secure_directory(parent)?;
    let mut input = File::open(source)
        .with_context(|| format!("could not open protected source {}", source.display()))?;
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut output = options
        .open(destination)
        .with_context(|| format!("could not create {}", destination.display()))?;
    let result = (|| -> Result<()> {
        #[cfg(windows)]
        windows_acl::apply_owner_admin_system_acl(destination)?;
        std::io::copy(&mut input, &mut output).context("could not copy protected file")?;
        output
            .sync_all()
            .context("could not flush protected copy")?;
        #[cfg(windows)]
        windows_acl::apply_owner_admin_system_acl(destination)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(destination);
    }
    result.map(|_| ())
}

pub fn validate_backup_name(name: &str) -> Result<()> {
    if name.is_empty()
        || name.len() > 96
        || name == "."
        || name == ".."
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        bail!("backup name must contain only letters, numbers, dot, underscore, and hyphen");
    }
    Ok(())
}

pub fn create_new_secure_directory(path: &Path) -> Result<()> {
    let parent = path
        .parent()
        .context("new protected directory has no parent")?;
    ensure_secure_directory(parent)?;
    if path.exists() {
        bail!("protected output {} already exists", path.display());
    }
    fs::create_dir(path)
        .with_context(|| format!("could not create protected output {}", path.display()))?;
    #[cfg(unix)]
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    #[cfg(windows)]
    if let Err(error) = windows_acl::apply_owner_admin_system_acl(path) {
        let _ = fs::remove_dir(path);
        return Err(error);
    }
    Ok(())
}

pub fn reject_symlinks_in_existing_path(path: &Path) -> Result<()> {
    let mut current = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => current.push(prefix.as_os_str()),
            Component::RootDir => current.push(component.as_os_str()),
            Component::Normal(part) => current.push(part),
            Component::CurDir | Component::ParentDir => {
                bail!("protected path must not contain traversal components")
            }
        }
        match fs::symlink_metadata(&current) {
            Ok(metadata)
                if metadata.file_type().is_symlink()
                    && !is_trusted_platform_root_link(&current) =>
            {
                bail!(
                    "protected path must not traverse symlink {}",
                    current.display()
                )
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("could not inspect {}", current.display()))
            }
        }
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn is_trusted_platform_root_link(path: &Path) -> bool {
    matches!(
        (path.to_str(), fs::read_link(path).ok().as_deref()),
        (Some("/var"), Some(target)) if target == Path::new("private/var")
    ) || matches!(
        (path.to_str(), fs::read_link(path).ok().as_deref()),
        (Some("/tmp"), Some(target)) if target == Path::new("private/tmp")
    ) || matches!(
        (path.to_str(), fs::read_link(path).ok().as_deref()),
        (Some("/etc"), Some(target)) if target == Path::new("private/etc")
    )
}

#[cfg(not(target_os = "macos"))]
fn is_trusted_platform_root_link(_path: &Path) -> bool {
    false
}

pub struct OperationLock {
    path: PathBuf,
}

impl OperationLock {
    pub fn acquire(root: &Path) -> Result<Self> {
        ensure_secure_directory(root)?;
        let path = root.join("operation.lock");
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        options.mode(0o600);
        let mut file = options.open(&path).with_context(|| {
            format!(
                "another operation may be active (lock {} exists); remove it only after confirming no talos-server process is running",
                path.display()
            )
        })?;
        #[cfg(windows)]
        if let Err(error) = windows_acl::apply_owner_admin_system_acl(&path) {
            drop(file);
            let _ = fs::remove_file(&path);
            return Err(error);
        }
        writeln!(file, "pid={}", std::process::id()).context("could not write operation lock")?;
        file.sync_all().context("could not flush operation lock")?;
        Ok(Self { path })
    }
}

impl Drop for OperationLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn temporary_sibling(path: &Path) -> Result<PathBuf> {
    let file_name = path
        .file_name()
        .context("protected file path has no file name")?;
    let mut random = [0_u8; 8];
    OsRng.fill_bytes(&mut random);
    let suffix = random
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let mut name = OsString::from(".");
    name.push(file_name);
    name.push(format!(".{suffix}.tmp"));
    Ok(path.with_file_name(name))
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> Result<()> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .with_context(|| format!("could not flush directory {}", path.display()))
}

#[cfg(not(unix))]
fn sync_directory(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(windows)]
mod windows_acl {
    use std::{ffi::c_void, os::windows::ffi::OsStrExt, path::Path, ptr};

    use anyhow::{bail, Result};
    use windows_sys::Win32::{
        Foundation::{GetLastError, LocalFree, ERROR_SUCCESS},
        Security::{
            Authorization::{
                ConvertStringSecurityDescriptorToSecurityDescriptorW, SetNamedSecurityInfoW,
                SDDL_REVISION_1, SE_FILE_OBJECT,
            },
            GetSecurityDescriptorDacl, DACL_SECURITY_INFORMATION,
            PROTECTED_DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR,
        },
    };

    pub(super) fn apply_owner_admin_system_acl(path: &Path) -> Result<()> {
        // Protected DACL: object owner, BUILTIN\Administrators, and SYSTEM receive full control.
        // OI/CI makes the allowlist inherit below a state root.
        let mut sddl: Vec<u16> = super::WINDOWS_PROTECTED_SDDL.encode_utf16().collect();
        sddl.push(0);
        let mut descriptor: PSECURITY_DESCRIPTOR = ptr::null_mut();
        // SAFETY: `sddl` is NUL-terminated and alive for the call; `descriptor` is a valid out
        // pointer. On success Windows allocates the descriptor with LocalAlloc, and the guard
        // below releases it exactly once with LocalFree.
        let converted = unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                SDDL_REVISION_1,
                &mut descriptor,
                ptr::null_mut(),
            )
        };
        if converted == 0 || descriptor.is_null() {
            // SAFETY: GetLastError has no preconditions and is read immediately after failure.
            let error = unsafe { GetLastError() };
            bail!("could not construct the protected Windows ACL (error {error})");
        }
        let descriptor = LocalDescriptor(descriptor);

        let mut present = 0;
        let mut defaulted = 0;
        let mut dacl = ptr::null_mut();
        // SAFETY: the guarded descriptor is valid; all remaining parameters are initialized out
        // pointers. `dacl` points inside the descriptor and stays alive until SetNamedSecurityInfoW
        // returns.
        let got_dacl = unsafe {
            GetSecurityDescriptorDacl(descriptor.0, &mut present, &mut dacl, &mut defaulted)
        };
        if got_dacl == 0 || present == 0 || dacl.is_null() {
            // SAFETY: GetLastError has no preconditions and is read immediately after failure.
            let error = unsafe { GetLastError() };
            bail!("could not read the protected Windows DACL (error {error})");
        }

        let mut wide_path: Vec<u16> = path.as_os_str().encode_wide().collect();
        if wide_path.contains(&0) {
            bail!("protected Windows path contains an embedded NUL");
        }
        wide_path.push(0);
        // SAFETY: `wide_path` is NUL-terminated and alive for the call; `dacl` is owned by the
        // guarded descriptor. Owner/group/SACL pointers are null because only the protected DACL
        // is being replaced.
        let result = unsafe {
            SetNamedSecurityInfoW(
                wide_path.as_ptr(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                ptr::null_mut(),
                ptr::null_mut(),
                dacl,
                ptr::null_mut(),
            )
        };
        if result != ERROR_SUCCESS {
            bail!(
                "could not apply owner/Administrators/SYSTEM-only ACL to {} (error {})",
                path.display(),
                result
            );
        }
        Ok(())
    }

    struct LocalDescriptor(PSECURITY_DESCRIPTOR);

    impl Drop for LocalDescriptor {
        fn drop(&mut self) {
            // SAFETY: this pointer is returned by
            // ConvertStringSecurityDescriptorToSecurityDescriptorW and is freed exactly once.
            unsafe {
                LocalFree(self.0.cast::<c_void>());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn atomic_write_creates_restrictive_file_and_replaces_content() {
        let temporary = TempDir::new().expect("tempdir");
        let root = temporary.path().join("state");
        let path = root.join("protected.json");
        atomic_write(&path, b"one").expect("first write");
        atomic_write(&path, b"two").expect("replacement");
        assert_eq!(read_protected_file(&path, 16).expect("read"), b"two");

        #[cfg(unix)]
        assert_eq!(
            fs::metadata(&path).expect("metadata").permissions().mode() & 0o777,
            0o600
        );
    }

    #[test]
    fn lock_prevents_concurrent_mutation_and_is_removed_on_drop() {
        let temporary = TempDir::new().expect("tempdir");
        let root = temporary.path().join("state");
        let first = OperationLock::acquire(&root).expect("first lock");
        assert!(OperationLock::acquire(&root).is_err());
        drop(first);
        OperationLock::acquire(&root).expect("lock after release");
    }

    #[cfg(unix)]
    #[test]
    fn protected_reader_rejects_world_readable_and_symlinked_files() {
        use std::os::unix::fs::symlink;

        let temporary = TempDir::new().expect("tempdir");
        let source = temporary.path().join("source");
        fs::write(&source, b"secret").expect("write");
        fs::set_permissions(&source, fs::Permissions::from_mode(0o644)).expect("chmod");
        assert!(read_protected_file(&source, 100).is_err());

        fs::set_permissions(&source, fs::Permissions::from_mode(0o600)).expect("chmod");
        let link = temporary.path().join("link");
        symlink(&source, &link).expect("symlink");
        assert!(read_protected_file(&link, 100).is_err());
    }

    #[test]
    fn backup_names_cannot_escape_the_configured_directory() {
        assert!(validate_backup_name("daily-2026.08.28").is_ok());
        assert!(validate_backup_name("../escape").is_err());
        assert!(validate_backup_name("a/b").is_err());
        assert!(validate_backup_name("..").is_err());
    }

    #[test]
    fn windows_acl_contract_is_protected_and_has_only_expected_trustees() {
        assert!(WINDOWS_PROTECTED_SDDL.starts_with("D:P"));
        let trustees: Vec<_> = WINDOWS_PROTECTED_SDDL
            .split(";;;")
            .skip(1)
            .filter_map(|part| part.split(')').next())
            .collect();
        assert_eq!(trustees, ["OW", "BA", "SY"]);
        assert!(!WINDOWS_PROTECTED_SDDL.contains("WD"));
        assert!(!WINDOWS_PROTECTED_SDDL.contains("AU"));
        assert!(!WINDOWS_PROTECTED_SDDL.contains("BU"));
    }
}
