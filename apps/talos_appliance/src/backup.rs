use std::{
    fs::File,
    io::Read,
    path::{Component, Path, PathBuf},
};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{
    compose::{combine_operation_and_cleanup, ComposeProject},
    config::{DatabaseConfig, EdgeMode, InstallationConfig, SecretConfig, CONFIG_SCHEMA_VERSION},
    process::CommandExecutor,
    secure_fs,
    state::{now_unix, DeploymentState, STATE_SCHEMA_VERSION},
};

pub const BACKUP_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackupManifest {
    pub schema_version: u32,
    pub name: String,
    pub created_at_unix: u64,
    pub installation_id: String,
    pub config_schema_version: u32,
    pub state_schema_version: u32,
    pub release_version: String,
    pub database: BackupDatabase,
    pub edge_state_included: bool,
    pub files: Vec<BackupFile>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum BackupDatabase {
    BundledPostgres { major_version: u16 },
    ExternalOperatorSupplied,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackupFile {
    pub path: String,
    pub sha256: String,
    pub bytes: u64,
}

pub fn create_backup(
    name: Option<&str>,
    external_database_backup: Option<&Path>,
    config: &InstallationConfig,
    secrets: &SecretConfig,
    state: &DeploymentState,
    project: &ComposeProject,
    executor: &dyn CommandExecutor,
    include_edge_state: bool,
) -> Result<(String, PathBuf)> {
    let name = name
        .map(ToString::to_string)
        .unwrap_or_else(|| format!("backup-{}", now_unix()));
    secure_fs::validate_backup_name(&name)?;
    secure_fs::ensure_secure_directory(&config.backup_directory)?;
    let output = config.backup_directory.join(&name);
    secure_fs::create_new_secure_directory(&output)?;

    let result = (|| -> Result<BackupManifest> {
        secure_fs::copy_protected_file(
            &config.installation_root.join("config.json"),
            &output.join("config.json"),
            secure_fs::MAX_STATE_FILE_BYTES,
        )?;
        secure_fs::copy_protected_file(
            &config.installation_root.join("secrets.json"),
            &output.join("secrets.json"),
            secure_fs::MAX_STATE_FILE_BYTES,
        )?;
        secure_fs::copy_protected_file(
            &config.installation_root.join("state.json"),
            &output.join("state.json"),
            secure_fs::MAX_STATE_FILE_BYTES,
        )?;

        let database = match &config.database {
            DatabaseConfig::Bundled { user, database } => {
                let dump = output.join("database.dump");
                project.postgres_dump(
                    executor,
                    user,
                    database,
                    &dump,
                    &secrets.redaction_values(),
                )?;
                secure_fs::harden_regular_file(&dump)?;
                if dump.metadata()?.len() == 0 {
                    bail!("PostgreSQL produced an empty logical backup");
                }
                project.verify_postgres_dump(executor, &dump, &secrets.redaction_values())?;
                BackupDatabase::BundledPostgres { major_version: 16 }
            }
            DatabaseConfig::External { .. } => {
                let source = external_database_backup.context(
                    "external PostgreSQL mode requires --external-database-backup pointing to a provider-verified backup",
                )?;
                if !source.is_absolute() {
                    bail!("external database backup path must be absolute");
                }
                secure_fs::copy_large_protected_file(source, &output.join("database.dump"))?;
                if output.join("database.dump").metadata()?.len() == 0 {
                    bail!("external database backup is empty");
                }
                BackupDatabase::ExternalOperatorSupplied
            }
        };

        match config.edge.mode {
            EdgeMode::PublicAcme => {
                if include_edge_state {
                    let acme = output.join("acme.json");
                    let was_running = state.lifecycle == crate::state::Lifecycle::Running;
                    if was_running {
                        project.stop_traefik(executor, &secrets.redaction_values())?;
                    }
                    let copy_result =
                        project.copy_from_acme_volume(executor, &acme, &secrets.redaction_values());
                    let restart_result = if was_running {
                        project.restart_traefik(executor, &secrets.redaction_values())
                    } else {
                        Ok(())
                    };
                    combine_operation_and_cleanup(
                        copy_result,
                        restart_result,
                        "Traefik restart after ACME backup",
                    )?;
                    secure_fs::harden_regular_file(&acme)?;
                }
            }
            EdgeMode::Local => {
                secure_fs::copy_protected_file(
                    &config
                        .installation_root
                        .join("local-tls")
                        .join("certificate.pem"),
                    &output.join("local-certificate.pem"),
                    1024 * 1024,
                )?;
                secure_fs::copy_protected_file(
                    &config
                        .installation_root
                        .join("local-tls")
                        .join("private-key.pem"),
                    &output.join("local-private-key.pem"),
                    1024 * 1024,
                )?;
                secure_fs::copy_protected_file(
                    &config
                        .installation_root
                        .join("local-tls")
                        .join("metadata.json"),
                    &output.join("local-certificate-metadata.json"),
                    1024 * 1024,
                )?;
            }
            EdgeMode::CustomCertificate => {}
        }

        let mut files = Vec::new();
        for name in [
            "config.json",
            "secrets.json",
            "state.json",
            "database.dump",
            "acme.json",
            "local-certificate.pem",
            "local-private-key.pem",
            "local-certificate-metadata.json",
        ] {
            let path = output.join(name);
            if path.exists() {
                files.push(file_record(&path, name)?);
            }
        }
        Ok(BackupManifest {
            schema_version: BACKUP_SCHEMA_VERSION,
            name: name.clone(),
            created_at_unix: now_unix(),
            installation_id: state.installation_id.clone(),
            config_schema_version: config.schema_version,
            state_schema_version: state.schema_version,
            release_version: config.release_version.clone(),
            database,
            edge_state_included: config.edge.mode != EdgeMode::PublicAcme || include_edge_state,
            files,
        })
    })();

    match result {
        Ok(manifest) => {
            secure_fs::atomic_write_json(&output.join("manifest.json"), &manifest)?;
            verify_backup(&output, Some(&state.installation_id))?;
            Ok((name, output))
        }
        Err(error) => {
            let failed = output.with_file_name(format!("{name}.incomplete-{}", now_unix()));
            let _ = std::fs::rename(&output, &failed);
            Err(error).context(
                "backup failed; any partial output was retained with an .incomplete suffix",
            )
        }
    }
}

pub fn verify_backup(
    path: &Path,
    expected_installation_id: Option<&str>,
) -> Result<BackupManifest> {
    secure_fs::reject_symlinks_in_existing_path(path)?;
    let manifest: BackupManifest = secure_fs::read_json(&path.join("manifest.json"))?;
    if manifest.schema_version != BACKUP_SCHEMA_VERSION
        || manifest.config_schema_version != CONFIG_SCHEMA_VERSION
        || manifest.state_schema_version != STATE_SCHEMA_VERSION
    {
        bail!("backup uses an unsupported schema version");
    }
    if path.file_name().and_then(|name| name.to_str()) != Some(&manifest.name) {
        bail!("backup directory name does not match its manifest");
    }
    if let Some(expected) = expected_installation_id {
        if manifest.installation_id != expected {
            bail!("backup belongs to a different Talos installation");
        }
    }
    if manifest.files.is_empty() {
        bail!("backup manifest contains no files");
    }
    for record in &manifest.files {
        validate_relative_backup_path(&record.path)?;
        let file = path.join(&record.path);
        let actual = file_record(&file, &record.path)?;
        if actual.sha256 != record.sha256 || actual.bytes != record.bytes {
            bail!("backup integrity verification failed for {}", record.path);
        }
    }
    for required in ["config.json", "secrets.json", "state.json", "database.dump"] {
        if !manifest.files.iter().any(|file| file.path == required) {
            bail!("backup manifest is missing {required}");
        }
    }
    Ok(manifest)
}

fn validate_relative_backup_path(value: &str) -> Result<()> {
    let path = Path::new(value);
    if path.is_absolute()
        || path
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        bail!("backup manifest contains an unsafe file path");
    }
    Ok(())
}

fn file_record(path: &Path, relative: &str) -> Result<BackupFile> {
    secure_fs::reject_symlinks_in_existing_path(path)?;
    let mut file = File::open(path)
        .with_context(|| format!("could not open backup file {}", path.display()))?;
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        bail!("backup entry {} is not a regular file", path.display());
    }
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(BackupFile {
        path: relative.to_string(),
        sha256: format!("{:x}", hasher.finalize()),
        bytes: metadata.len(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::anyhow;
    use tempfile::TempDir;

    #[test]
    fn detects_backup_tampering_and_manifest_traversal() {
        let temporary = TempDir::new().expect("tempdir");
        let root = temporary.path().join("backup-1");
        secure_fs::create_new_secure_directory(&root).expect("directory");
        for name in ["config.json", "secrets.json", "state.json", "database.dump"] {
            secure_fs::atomic_write(&root.join(name), format!("{name}-content").as_bytes())
                .expect("file");
        }
        let files = ["config.json", "secrets.json", "state.json", "database.dump"]
            .into_iter()
            .map(|name| file_record(&root.join(name), name).expect("record"))
            .collect();
        let manifest = BackupManifest {
            schema_version: 1,
            name: "backup-1".to_string(),
            created_at_unix: 1,
            installation_id: "a".repeat(32),
            config_schema_version: 1,
            state_schema_version: 1,
            release_version: "1.0.0".to_string(),
            database: BackupDatabase::BundledPostgres { major_version: 16 },
            edge_state_included: true,
            files,
        };
        secure_fs::atomic_write_json(&root.join("manifest.json"), &manifest).expect("manifest");
        verify_backup(&root, Some(&"a".repeat(32))).expect("valid backup");

        secure_fs::atomic_write(&root.join("database.dump"), b"tampered").expect("tamper");
        assert!(verify_backup(&root, Some(&"a".repeat(32))).is_err());

        let mut unsafe_manifest = manifest;
        unsafe_manifest.files[0].path = "../escape".to_string();
        secure_fs::atomic_write_json(&root.join("manifest.json"), &unsafe_manifest)
            .expect("unsafe manifest");
        assert!(verify_backup(&root, Some(&"a".repeat(32))).is_err());
    }

    #[test]
    fn acme_backup_preserves_copy_and_edge_restart_failures() {
        let error = combine_operation_and_cleanup::<()>(
            Err(anyhow!("ACME copy failed")),
            Err(anyhow!(
                "Traefik restart failed; public ingress remains stopped"
            )),
            "Traefik restart after ACME backup",
        )
        .expect_err("both failures must be surfaced");
        let detail = format!("{error:#}");
        assert!(detail.contains("ACME copy failed"));
        assert!(detail.contains("Traefik restart after ACME backup"));
        assert!(detail.contains("public ingress remains stopped"));
    }
}
