use std::{
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{bail, Context, Result};
use rand::{rngs::OsRng, RngCore};
use serde::{Deserialize, Serialize};

use crate::{config::InstallationConfig, secure_fs};

pub const STATE_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeploymentState {
    pub schema_version: u32,
    pub installation_id: String,
    pub lifecycle: Lifecycle,
    pub current: Option<DeploymentVersion>,
    pub previous_good: Option<DeploymentVersion>,
    pub operation: Option<OperationJournal>,
    pub last_verified_backup: Option<String>,
    pub last_transition_unix: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeploymentVersion {
    pub config: InstallationConfig,
    pub traefik: ResolvedImage,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ResolvedImage {
    pub digest: String,
    pub version: String,
    pub resolved_at_unix: u64,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Lifecycle {
    Installing,
    Running,
    Stopped,
    FailedRecoverable,
    RestoreRequired,
    UninstalledDataPreserved,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OperationJournal {
    pub kind: OperationKind,
    pub checkpoint: Checkpoint,
    pub started_at_unix: u64,
    pub backup_name: Option<String>,
    pub application_images_changed: bool,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OperationKind {
    Install,
    Update,
    Restore,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum Checkpoint {
    ConfigurationStored,
    AssetsMaterialized,
    PrerequisitesReady,
    ImagesResolved,
    DatabaseReady,
    PreflightPassed,
    BackupVerified,
    MigrationStarted,
    MigrationCompleted,
    ServicesStarted,
    EdgeReady,
    Healthy,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RollbackDecision {
    RestorePreviousConfigurationAndRestart,
    RollBackTraefikOnly,
    StopAndRestoreVerifiedBackup,
}

impl DeploymentState {
    pub fn new_install(config: InstallationConfig) -> Self {
        Self {
            schema_version: STATE_SCHEMA_VERSION,
            installation_id: random_id(),
            lifecycle: Lifecycle::Installing,
            current: None,
            previous_good: None,
            operation: Some(OperationJournal {
                kind: OperationKind::Install,
                checkpoint: Checkpoint::ConfigurationStored,
                started_at_unix: now_unix(),
                backup_name: None,
                application_images_changed: true,
            }),
            last_verified_backup: None,
            last_transition_unix: now_unix(),
        }
        .with_pending_config(config)
    }

    fn with_pending_config(mut self, config: InstallationConfig) -> Self {
        self.current = Some(DeploymentVersion {
            config,
            traefik: ResolvedImage {
                digest: String::new(),
                version: String::new(),
                resolved_at_unix: 0,
            },
        });
        self
    }

    pub fn load(path: &Path, expected_root: &Path) -> Result<Self> {
        let state: Self = secure_fs::read_json(path)?;
        state.validate(expected_root)?;
        Ok(state)
    }

    pub fn save(&mut self, path: &Path) -> Result<()> {
        self.last_transition_unix = now_unix();
        secure_fs::atomic_write_json(path, self)
    }

    pub fn validate(&self, expected_root: &Path) -> Result<()> {
        if self.schema_version != STATE_SCHEMA_VERSION {
            bail!("unsupported deployment-state schema");
        }
        if self.installation_id.len() != 32
            || !self
                .installation_id
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
        {
            bail!("deployment state contains an invalid installation identifier");
        }
        if let Some(current) = &self.current {
            if current.config.installation_root != expected_root {
                bail!(
                    "deployment state belongs to a different installation root; pass the original --state-dir"
                );
            }
            current.config.images.validate()?;
            if !current.traefik.digest.is_empty() {
                crate::config::validate_digest_image("Traefik", &current.traefik.digest)?;
            }
        }
        if let Some(previous) = &self.previous_good {
            previous.config.images.validate()?;
            crate::config::validate_digest_image("previous Traefik", &previous.traefik.digest)?;
        }
        Ok(())
    }

    pub fn pending_version(&self) -> Result<&DeploymentVersion> {
        self.current
            .as_ref()
            .context("deployment state has no configured version")
    }

    pub fn checkpoint(&mut self, next: Checkpoint) -> Result<()> {
        let operation = self
            .operation
            .as_mut()
            .context("cannot checkpoint without an active operation")?;
        if next < operation.checkpoint {
            bail!("operation checkpoint cannot move backwards");
        }
        operation.checkpoint = next;
        Ok(())
    }

    pub fn begin_update(
        &mut self,
        candidate: DeploymentVersion,
        backup_name: String,
        application_change: bool,
    ) -> Result<()> {
        if self.operation.is_some() {
            bail!("cannot begin update while another operation is incomplete");
        }
        let previous = self
            .current
            .replace(candidate)
            .context("cannot update an installation without a current version")?;
        self.previous_good = Some(previous);
        self.operation = Some(OperationJournal {
            kind: OperationKind::Update,
            checkpoint: Checkpoint::BackupVerified,
            started_at_unix: now_unix(),
            backup_name: Some(backup_name),
            application_images_changed: application_change,
        });
        self.lifecycle = Lifecycle::Installing;
        Ok(())
    }

    pub fn begin_restore(&mut self, backup_name: String) -> Result<()> {
        if self.operation.is_some() {
            bail!("cannot begin restore while another operation is incomplete");
        }
        self.operation = Some(OperationJournal {
            kind: OperationKind::Restore,
            checkpoint: Checkpoint::BackupVerified,
            started_at_unix: now_unix(),
            backup_name: Some(backup_name),
            application_images_changed: true,
        });
        self.lifecycle = Lifecycle::Installing;
        Ok(())
    }

    pub fn promote_healthy(&mut self) -> Result<()> {
        self.checkpoint(Checkpoint::Healthy)?;
        self.lifecycle = Lifecycle::Running;
        self.operation = None;
        Ok(())
    }

    pub fn mark_stopped(&mut self) {
        self.lifecycle = Lifecycle::Stopped;
        self.operation = None;
    }

    pub fn mark_recoverable_failure(&mut self) {
        self.lifecycle = Lifecycle::FailedRecoverable;
        self.operation = None;
    }

    pub fn mark_restore_required(&mut self) {
        self.lifecycle = Lifecycle::RestoreRequired;
    }

    pub fn rollback_decision(&self) -> Result<RollbackDecision> {
        let operation = self
            .operation
            .as_ref()
            .context("no active operation requires rollback")?;
        Ok(rollback_decision(
            operation.checkpoint,
            operation.application_images_changed,
        ))
    }

    pub fn restore_previous_version(&mut self) -> Result<()> {
        let previous = self
            .previous_good
            .take()
            .context("no previous known-good deployment is recorded")?;
        self.current = Some(previous);
        self.operation = None;
        self.lifecycle = Lifecycle::FailedRecoverable;
        Ok(())
    }

    pub fn state_path(root: &Path) -> PathBuf {
        root.join("state.json")
    }
}

pub fn rollback_decision(
    checkpoint: Checkpoint,
    application_images_changed: bool,
) -> RollbackDecision {
    if !application_images_changed {
        RollbackDecision::RollBackTraefikOnly
    } else if checkpoint < Checkpoint::MigrationStarted {
        RollbackDecision::RestorePreviousConfigurationAndRestart
    } else {
        RollbackDecision::StopAndRestoreVerifiedBackup
    }
}

pub fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn random_id() -> String {
    let mut bytes = [0_u8; 16];
    OsRng.fill_bytes(&mut bytes);
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rollback_is_automatic_only_before_migration_may_have_changed_schema() {
        assert_eq!(
            rollback_decision(Checkpoint::PreflightPassed, true),
            RollbackDecision::RestorePreviousConfigurationAndRestart
        );
        assert_eq!(
            rollback_decision(Checkpoint::MigrationStarted, true),
            RollbackDecision::StopAndRestoreVerifiedBackup
        );
        assert_eq!(
            rollback_decision(Checkpoint::ServicesStarted, false),
            RollbackDecision::RollBackTraefikOnly
        );
    }

    #[test]
    fn checkpoint_cannot_move_backwards() {
        let mut state = DeploymentState {
            schema_version: 1,
            installation_id: "a".repeat(32),
            lifecycle: Lifecycle::Installing,
            current: None,
            previous_good: None,
            operation: Some(OperationJournal {
                kind: OperationKind::Install,
                checkpoint: Checkpoint::ImagesResolved,
                started_at_unix: 1,
                backup_name: None,
                application_images_changed: true,
            }),
            last_verified_backup: None,
            last_transition_unix: 1,
        };
        assert!(state.checkpoint(Checkpoint::AssetsMaterialized).is_err());
    }
}
