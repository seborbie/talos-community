//! Downloaded-bundle startup reuses the journaled install/start operations.

use std::{fs, path::Path};

use anyhow::{bail, Context, Result};

use crate::{
    config::{DatabaseConfig, EdgeMode, InstallRequest, InstallationConfig},
    images::detect_docker,
    orchestrator,
    process::CommandExecutor,
    state::{Checkpoint, DeploymentState, Lifecycle, OperationKind},
};

#[derive(Debug, PartialEq, Eq)]
enum Action {
    Install,
    Start,
}

fn select_action(config: &InstallationConfig, state: Option<&DeploymentState>) -> Result<Action> {
    if config.edge.mode != EdgeMode::Local
        || !matches!(config.database, DatabaseConfig::Bundled { .. })
        || [
            &config.edge.frontend_domain,
            &config.edge.api_domain,
            &config.edge.control_domain,
            &config.edge.relay_domain,
        ]
        .iter()
        .any(|domain| !domain.ends_with(".localhost"))
    {
        bail!("quickstart requires the bundled database and local .localhost routes; use install for a custom deployment");
    }
    let Some(state) = state else {
        return Ok(Action::Install);
    };
    if state.lifecycle == Lifecycle::RestoreRequired {
        bail!("Talos requires backup recovery; inspect status and use restore before starting");
    }
    if &state.pending_version()?.config != config {
        bail!("this download differs from the installed configuration; use the original bundle to start, or review an explicit update --config request (updates require a backup)");
    }
    if let Some(operation) = &state.operation {
        if operation.kind == OperationKind::Install
            && operation.checkpoint < Checkpoint::MigrationStarted
        {
            return Ok(Action::Install);
        }
        bail!("an interrupted operation may have changed the database; inspect status and follow the recovery guide before retrying");
    }
    Ok(Action::Start)
}

pub fn run(
    root: &Path,
    docker_path: Option<&Path>,
    request: &Path,
    executor: &dyn CommandExecutor,
) -> Result<()> {
    // Check the runtime before installing secrets or creating a journal. Missing/stopped Docker
    // must leave a fresh installation untouched and safe to retry.
    println!("Checking Docker Engine and Compose (Linux containers required)...");
    detect_docker(executor, docker_path)
        .context("Install/start Docker with Compose v2 or newer, then retry. On Windows select Linux containers. Talos does not install Docker or change its permissions")?;
    let (config, _) = InstallRequest::load(request, root)?;
    let state_path = DeploymentState::state_path(root);
    let state = match fs::symlink_metadata(&state_path) {
        Ok(_) => Some(DeploymentState::load(&state_path, root)?),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error).context("could not inspect existing Talos installation"),
    };
    match select_action(&config, state.as_ref())? {
        Action::Install => {
            println!("Preparing Talos: pulling release images, generating private credentials, and preparing PostgreSQL. First startup can take several minutes.");
            orchestrator::install(root, docker_path, request, None, executor)
        }
        Action::Start => {
            println!(
                "Starting the installed Talos version with its saved credentials and database..."
            );
            orchestrator::start(root, docker_path, executor)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::process::{CommandSpec, ProcessOutput};
    use std::time::Duration;

    fn config(root: &Path) -> InstallationConfig {
        let mut request: serde_json::Value =
            serde_json::from_str(include_str!("../talos-server.example.json"))
                .expect("example JSON");
        request["edge"] = serde_json::json!({
            "mode": "local", "frontend_domain": "talos.localhost",
            "api_domain": "api.talos.localhost", "control_domain": "control.talos.localhost",
            "relay_domain": "relay.talos.localhost", "http_port": 8080, "https_port": 8443
        });
        serde_json::from_value::<InstallRequest>(request)
            .expect("request")
            .validate_and_split(root)
            .expect("valid local request")
            .0
    }

    #[test]
    fn first_run_and_safe_install_retry_use_existing_install_journal() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let config = config(temporary.path());
        assert_eq!(
            select_action(&config, None).expect("fresh"),
            Action::Install
        );
        let state = DeploymentState::new_install(config.clone());
        assert_eq!(
            select_action(&config, Some(&state)).expect("retry"),
            Action::Install
        );
    }

    #[test]
    fn repeated_run_never_updates_or_replaces_secrets() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let config = config(temporary.path());
        let mut state = DeploymentState::new_install(config.clone());
        state.operation = None;
        state.lifecycle = Lifecycle::Stopped;
        assert_eq!(
            select_action(&config, Some(&state)).expect("restart"),
            Action::Start
        );
        let mut changed = config.clone();
        changed.release_version = "9.0.0".to_string();
        assert!(select_action(&changed, Some(&state)).is_err());
        changed = config.clone();
        changed.images.frontend = format!("ghcr.io/example/frontend@sha256:{}", "f".repeat(64));
        assert!(select_action(&changed, Some(&state)).is_err());
        state.lifecycle = Lifecycle::RestoreRequired;
        assert!(select_action(&config, Some(&state)).is_err());
    }

    #[test]
    fn refuses_migration_retries_updates_and_nonlocal_configuration() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let mut config = config(temporary.path());
        let mut state = DeploymentState::new_install(config.clone());
        state
            .checkpoint(Checkpoint::MigrationStarted)
            .expect("checkpoint");
        assert!(select_action(&config, Some(&state)).is_err());
        state.operation.as_mut().expect("operation").kind = OperationKind::Update;
        assert!(select_action(&config, Some(&state)).is_err());
        config.edge.mode = EdgeMode::PublicAcme;
        assert!(select_action(&config, None).is_err());
        config.edge.mode = EdgeMode::Local;
        config.edge.frontend_domain = "talos.example.com".to_string();
        assert!(select_action(&config, None).is_err());
    }

    struct UnavailableDocker;
    impl CommandExecutor for UnavailableDocker {
        fn execute(&self, _: &CommandSpec, _: Duration) -> Result<ProcessOutput> {
            bail!("Docker unavailable")
        }
        fn execute_to_file(&self, _: &CommandSpec, _: Duration, _: &Path) -> Result<ProcessOutput> {
            bail!("unexpected file command")
        }
        fn execute_with_input(
            &self,
            _: &CommandSpec,
            _: Duration,
            _: &Path,
        ) -> Result<ProcessOutput> {
            bail!("unexpected input command")
        }
    }

    #[test]
    fn missing_docker_leaves_first_run_without_state_or_secrets() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let root = temporary.path().join("not-created");
        let docker = temporary.path().join("docker");
        fs::write(&docker, b"fixture").expect("Docker fixture");
        let error = run(
            &root,
            Some(&docker),
            &root.join("request.json"),
            &UnavailableDocker,
        )
        .expect_err("unavailable Docker");
        assert!(format!("{error:#}").contains("Install/start Docker"));
        assert!(!root.exists());
    }
}
