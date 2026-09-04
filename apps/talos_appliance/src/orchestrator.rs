use std::{ffi::OsString, fs, path::Path};

use anyhow::{bail, Context, Result};

use crate::{
    backup::{self, BackupDatabase},
    cli::{Cli, CliCommand, HELP},
    compose::{self, ComposeProject},
    config::{DatabaseConfig, EdgeMode, InstallRequest, InstallationConfig, SecretConfig},
    diagnostics,
    images::{self, detect_docker, DockerRuntime},
    process::{CommandExecutor, SystemExecutor},
    secure_fs::{self, OperationLock},
    state::{
        Checkpoint, DeploymentState, DeploymentVersion, Lifecycle, OperationKind, RollbackDecision,
    },
};

pub fn run<I>(arguments: I) -> Result<()>
where
    I: IntoIterator<Item = OsString>,
{
    let cli = Cli::parse(arguments)?;
    if matches!(cli.command, CliCommand::Help) {
        print!("{HELP}");
        return Ok(());
    }
    ensure_supported_host()?;
    crate::config::validate_absolute_path("state directory", &cli.state_dir)?;
    let executor = SystemExecutor;
    match cli.command {
        CliCommand::Install {
            config,
            external_database_backup,
        } => install(
            &cli.state_dir,
            cli.docker_path.as_deref(),
            &config,
            external_database_backup.as_deref(),
            &executor,
        ),
        CliCommand::Start => start(&cli.state_dir, cli.docker_path.as_deref(), &executor),
        CliCommand::Stop => stop(&cli.state_dir, cli.docker_path.as_deref(), &executor),
        CliCommand::Status => status(&cli.state_dir, cli.docker_path.as_deref(), &executor),
        CliCommand::Update {
            config,
            external_database_backup,
        } => update(
            &cli.state_dir,
            cli.docker_path.as_deref(),
            &config,
            external_database_backup.as_deref(),
            &executor,
        ),
        CliCommand::Backup {
            name,
            external_database_backup,
        } => backup_command(
            &cli.state_dir,
            cli.docker_path.as_deref(),
            name.as_deref(),
            external_database_backup.as_deref(),
            &executor,
        ),
        CliCommand::Restore {
            backup_name,
            confirmation,
            external_database_restored,
        } => restore(
            &cli.state_dir,
            cli.docker_path.as_deref(),
            &backup_name,
            &confirmation,
            external_database_restored,
            &executor,
        ),
        CliCommand::Diagnostics { name } => diagnostics_command(
            &cli.state_dir,
            cli.docker_path.as_deref(),
            name.as_deref(),
            &executor,
        ),
        CliCommand::Uninstall {
            remove_data,
            confirmation,
        } => uninstall(
            &cli.state_dir,
            cli.docker_path.as_deref(),
            remove_data,
            confirmation.as_deref(),
            &executor,
        ),
        CliCommand::Help => unreachable!("help returned before dispatch"),
    }
}

fn ensure_supported_host() -> Result<()> {
    #[cfg(any(
        all(target_os = "linux", target_arch = "x86_64"),
        all(target_os = "windows", target_arch = "x86_64")
    ))]
    {
        Ok(())
    }
    #[cfg(not(any(
        all(target_os = "linux", target_arch = "x86_64"),
        all(target_os = "windows", target_arch = "x86_64")
    )))]
    {
        bail!("this Talos Community launcher release supports Linux x86-64 and Windows x64 only")
    }
}

fn install(
    root: &Path,
    docker_path: Option<&Path>,
    request_path: &Path,
    external_database_backup: Option<&Path>,
    executor: &dyn CommandExecutor,
) -> Result<()> {
    secure_fs::ensure_secure_directory(root)?;
    let _lock = OperationLock::acquire(root)?;
    let (requested_config, input_secrets) = InstallRequest::load(request_path, root)?;
    validate_custom_certificate_files(&requested_config)?;
    let state_path = DeploymentState::state_path(root);

    let (config, secrets, mut state) = if state_path.exists() {
        let state = DeploymentState::load(&state_path, root)?;
        let operation = state
            .operation
            .as_ref()
            .context("installation already exists; use update, start, or status")?;
        if operation.kind != OperationKind::Install {
            bail!("another incomplete operation must be recovered before install can continue");
        }
        if operation.checkpoint >= Checkpoint::MigrationStarted
            && operation.checkpoint < Checkpoint::Healthy
        {
            bail!(
                "an interrupted install may have changed the database schema; inspect status and restore or remove the incomplete installation explicitly"
            );
        }
        let config: InstallationConfig = secure_fs::read_json(&root.join("config.json"))?;
        if config != requested_config {
            bail!(
                "install retry configuration does not match the recorded incomplete installation"
            );
        }
        let secrets: SecretConfig = secure_fs::read_json(&root.join("secrets.json"))?;
        secrets.validate_for(&config)?;
        (config, secrets, state)
    } else {
        let secrets = SecretConfig::generate(&requested_config, input_secrets);
        secrets.validate_for(&requested_config)?;
        let mut state = DeploymentState::new_install(requested_config.clone());
        secure_fs::atomic_write_json(&root.join("config.json"), &requested_config)?;
        secure_fs::atomic_write_json(&root.join("secrets.json"), &secrets)?;
        state.save(&state_path)?;
        (requested_config, secrets, state)
    };

    compose::materialize_assets(root)?;
    compose::ensure_local_certificate(root, &config)?;
    advance_checkpoint(&mut state, &state_path, Checkpoint::AssetsMaterialized)?;

    let runtime = detect_docker(executor, docker_path)?;
    crate::network::validate_docker_network_availability(
        executor,
        &runtime.executable,
        &config.edge.subnet,
    )?;
    advance_checkpoint(&mut state, &state_path, Checkpoint::PrerequisitesReady)?;
    let existing_traefik = state.pending_version()?.traefik.clone();
    let traefik = if existing_traefik.digest.is_empty() {
        images::pull_release_images(executor, &runtime.executable, &config.images)?;
        images::resolve_traefik_latest(executor, &runtime.executable)?
    } else {
        images::ensure_recorded_images(
            executor,
            &runtime.executable,
            &config.images,
            &existing_traefik,
        )?;
        existing_traefik
    };
    state
        .current
        .as_mut()
        .context("install state lost its candidate version")?
        .traefik = traefik.clone();
    state.save(&state_path)?;
    compose::materialize_environment(root, &config, &secrets, &traefik)?;
    advance_checkpoint(&mut state, &state_path, Checkpoint::ImagesResolved)?;
    let project = ComposeProject::new(runtime.executable.clone(), root.to_path_buf(), &config);
    project.validate(executor, &secrets.redaction_values())?;

    if matches!(config.database, DatabaseConfig::Bundled { .. }) {
        project.up_database(executor, &secrets.redaction_values())?;
    }
    advance_checkpoint(&mut state, &state_path, Checkpoint::DatabaseReady)?;
    project.run_database_job(executor, "database_preflight", &secrets.redaction_values())?;
    advance_checkpoint(&mut state, &state_path, Checkpoint::PreflightPassed)?;

    if state
        .operation
        .as_ref()
        .is_some_and(|operation| operation.checkpoint < Checkpoint::BackupVerified)
    {
        let backup_name = format!("pre-install-migration-{}", &state.installation_id[..8]);
        let backup_path = config.backup_directory.join(&backup_name);
        if backup_path.exists() {
            backup::verify_backup(&backup_path, Some(&state.installation_id))?;
        } else {
            backup::create_backup(
                Some(&backup_name),
                external_database_backup,
                &config,
                &secrets,
                &state,
                &project,
                executor,
                false,
            )?;
        }
        state.last_verified_backup = Some(backup_name);
        advance_checkpoint(&mut state, &state_path, Checkpoint::BackupVerified)?;
    }

    advance_checkpoint(&mut state, &state_path, Checkpoint::MigrationStarted)?;
    if let Err(error) =
        project.run_database_job(executor, "database_migrate", &secrets.redaction_values())
    {
        state.mark_restore_required();
        state.save(&state_path)?;
        return Err(error).context(
            "database migration failed after its durable checkpoint; do not retry blindly",
        );
    }
    advance_checkpoint(&mut state, &state_path, Checkpoint::MigrationCompleted)?;
    if let Err(error) = start_and_verify(&project, executor, &config, &secrets) {
        state.mark_restore_required();
        state.save(&state_path)?;
        return Err(error).context("installation did not become healthy after migration");
    }
    advance_checkpoint(&mut state, &state_path, Checkpoint::ServicesStarted)?;
    advance_checkpoint(&mut state, &state_path, Checkpoint::EdgeReady)?;
    state.promote_healthy()?;
    state.save(&state_path)?;
    println!(
        "Talos {} is running. Installation ID: {}. Frontend: {}",
        config.release_version,
        state.installation_id,
        frontend_url(&config)
    );
    Ok(())
}

fn start(root: &Path, docker_path: Option<&Path>, executor: &dyn CommandExecutor) -> Result<()> {
    let _lock = OperationLock::acquire(root)?;
    let (config, secrets, mut state) = load_installation(root)?;
    require_no_incomplete_operation(&state)?;
    if state.lifecycle == Lifecycle::RestoreRequired {
        bail!("installation requires a verified backup restore before it can be started");
    }
    let runtime = detect_docker(executor, docker_path)?;
    let traefik = &state.pending_version()?.traefik;
    images::ensure_recorded_images(executor, &runtime.executable, &config.images, traefik)?;
    compose::materialize_assets(root)?;
    compose::ensure_local_certificate(root, &config)?;
    compose::materialize_environment(root, &config, &secrets, traefik)?;
    let project = ComposeProject::new(runtime.executable, root.to_path_buf(), &config);
    project.validate(executor, &secrets.redaction_values())?;
    start_and_verify(&project, executor, &config, &secrets)?;
    state.lifecycle = Lifecycle::Running;
    state.save(&DeploymentState::state_path(root))?;
    println!("Talos is running at {}", frontend_url(&config));
    Ok(())
}

fn stop(root: &Path, docker_path: Option<&Path>, executor: &dyn CommandExecutor) -> Result<()> {
    let _lock = OperationLock::acquire(root)?;
    let (config, secrets, mut state) = load_installation(root)?;
    require_no_incomplete_operation(&state)?;
    let runtime = detect_docker(executor, docker_path)?;
    let project = ComposeProject::new(runtime.executable, root.to_path_buf(), &config);
    project.stop(executor, &secrets.redaction_values())?;
    state.mark_stopped();
    state.save(&DeploymentState::state_path(root))?;
    println!("Talos is stopped. Durable data and protected configuration were preserved.");
    Ok(())
}

fn status(root: &Path, docker_path: Option<&Path>, executor: &dyn CommandExecutor) -> Result<()> {
    let (config, secrets, state) = load_installation(root)?;
    let runtime = detect_docker(executor, docker_path)?;
    let project = ComposeProject::new(runtime.executable, root.to_path_buf(), &config);
    let services =
        diagnostics::collect_service_status(&project, executor, &secrets.redaction_values())?;
    let output = serde_json::json!({
        "installation_id": state.installation_id,
        "lifecycle": format!("{:?}", state.lifecycle).to_ascii_lowercase(),
        "release_version": config.release_version,
        "frontend": frontend_url(&config),
        "operation": state.operation,
        "services": services,
        "last_verified_backup": state.last_verified_backup,
    });
    println!("{}", serde_json::to_string_pretty(&output)?);
    Ok(())
}

fn backup_command(
    root: &Path,
    docker_path: Option<&Path>,
    name: Option<&str>,
    external_database_backup: Option<&Path>,
    executor: &dyn CommandExecutor,
) -> Result<()> {
    let _lock = OperationLock::acquire(root)?;
    let (config, secrets, mut state) = load_installation(root)?;
    require_no_incomplete_operation(&state)?;
    let runtime = detect_docker(executor, docker_path)?;
    let project = ComposeProject::new(runtime.executable, root.to_path_buf(), &config);
    let bundled_database = matches!(config.database, DatabaseConfig::Bundled { .. });
    let restore_stopped_database = bundled_database
        && matches!(
            state.lifecycle,
            Lifecycle::Stopped | Lifecycle::UninstalledDataPreserved
        );
    let create = || {
        if bundled_database {
            project.up_database(executor, &secrets.redaction_values())?;
        }
        backup::create_backup(
            name,
            external_database_backup,
            &config,
            &secrets,
            &state,
            &project,
            executor,
            true,
        )
    };
    let (backup_name, path) = if restore_stopped_database {
        run_with_guaranteed_cleanup(
            create,
            || project.stop_database(executor, &secrets.redaction_values()),
            "temporary bundled PostgreSQL cleanup",
        )?
    } else {
        create()?
    };
    state.last_verified_backup = Some(backup_name);
    state.save(&DeploymentState::state_path(root))?;
    println!("Verified backup created at {}", path.display());
    Ok(())
}

fn update(
    root: &Path,
    docker_path: Option<&Path>,
    request_path: &Path,
    external_database_backup: Option<&Path>,
    executor: &dyn CommandExecutor,
) -> Result<()> {
    let _lock = OperationLock::acquire(root)?;
    let (current_config, current_secrets, mut state) = load_installation(root)?;
    if state.operation.is_some() {
        recover_interrupted_update(root, docker_path, executor, &current_secrets, &mut state)?;
        bail!("the interrupted update was recovered; review status, then run update again");
    }
    if state.lifecycle != Lifecycle::Running {
        bail!("update requires a healthy running installation");
    }
    let (candidate_config, secret_input) = InstallRequest::load(request_path, root)?;
    validate_custom_certificate_files(&candidate_config)?;
    ensure_database_identity_unchanged(&current_config.database, &candidate_config.database)?;
    if candidate_config.edge.subnet != current_config.edge.subnet
        || candidate_config.edge.proxy_ipv4 != current_config.edge.proxy_ipv4
    {
        bail!(
            "update cannot change the active edge subnet or proxy address; use a separately reviewed network migration"
        );
    }
    let mut candidate_secrets = current_secrets.clone();
    candidate_secrets.update_database_secret(&candidate_config, secret_input);
    candidate_secrets.validate_for(&candidate_config)?;

    let runtime = detect_docker(executor, docker_path)?;
    let current_project = ComposeProject::new(
        runtime.executable.clone(),
        root.to_path_buf(),
        &current_config,
    );
    let backup_name = format!(
        "pre-update-{}-{}",
        crate::state::now_unix(),
        &state.installation_id[..8]
    );
    let (backup_name, _) = backup::create_backup(
        Some(&backup_name),
        external_database_backup,
        &current_config,
        &current_secrets,
        &state,
        &current_project,
        executor,
        true,
    )?;
    state.last_verified_backup = Some(backup_name.clone());
    persist_rollback_snapshot(root, &current_config, &current_secrets)?;

    images::pull_release_images(executor, &runtime.executable, &candidate_config.images)?;
    let candidate_traefik = images::resolve_traefik_latest(executor, &runtime.executable)?;
    let candidate = DeploymentVersion {
        config: candidate_config.clone(),
        traefik: candidate_traefik.clone(),
    };
    let application_change = current_config.images != candidate_config.images
        || current_config.database != candidate_config.database
        || current_config.release_version != candidate_config.release_version;
    state.begin_update(candidate, backup_name, application_change)?;
    state.save(&DeploymentState::state_path(root))?;
    persist_active_configuration(root, &candidate_config, &candidate_secrets)?;
    compose::materialize_assets(root)?;
    compose::ensure_local_certificate(root, &candidate_config)?;
    compose::materialize_environment(
        root,
        &candidate_config,
        &candidate_secrets,
        &candidate_traefik,
    )?;
    advance_checkpoint(
        &mut state,
        &DeploymentState::state_path(root),
        Checkpoint::ImagesResolved,
    )?;
    let project = ComposeProject::new(
        runtime.executable.clone(),
        root.to_path_buf(),
        &candidate_config,
    );
    let update_result = (|| -> Result<()> {
        project.validate(executor, &candidate_secrets.redaction_values())?;
        if matches!(candidate_config.database, DatabaseConfig::Bundled { .. }) {
            project.up_database(executor, &candidate_secrets.redaction_values())?;
        }
        advance_checkpoint(
            &mut state,
            &DeploymentState::state_path(root),
            Checkpoint::DatabaseReady,
        )?;
        project.run_database_job(
            executor,
            "database_preflight",
            &candidate_secrets.redaction_values(),
        )?;
        advance_checkpoint(
            &mut state,
            &DeploymentState::state_path(root),
            Checkpoint::PreflightPassed,
        )?;
        advance_checkpoint(
            &mut state,
            &DeploymentState::state_path(root),
            Checkpoint::MigrationStarted,
        )?;
        project.run_database_job(
            executor,
            "database_migrate",
            &candidate_secrets.redaction_values(),
        )?;
        advance_checkpoint(
            &mut state,
            &DeploymentState::state_path(root),
            Checkpoint::MigrationCompleted,
        )?;
        start_and_verify(&project, executor, &candidate_config, &candidate_secrets)?;
        advance_checkpoint(
            &mut state,
            &DeploymentState::state_path(root),
            Checkpoint::ServicesStarted,
        )?;
        advance_checkpoint(
            &mut state,
            &DeploymentState::state_path(root),
            Checkpoint::EdgeReady,
        )?;
        Ok(())
    })();

    if let Err(error) = update_result {
        let recovery =
            recover_failed_update(root, &runtime, executor, &candidate_secrets, &mut state);
        return match recovery {
            Ok(message) => Err(error).context(message),
            Err(recovery_error) => Err(error).context(format!(
                "update failed and recovery also failed: {recovery_error:#}"
            )),
        };
    }
    state.promote_healthy()?;
    state.save(&DeploymentState::state_path(root))?;
    println!(
        "Talos updated to {}. Traefik resolved to {} ({}) and was promoted after health checks.",
        candidate_config.release_version, candidate_traefik.digest, candidate_traefik.version
    );
    Ok(())
}

fn restore(
    root: &Path,
    docker_path: Option<&Path>,
    backup_name: &str,
    confirmation: &str,
    external_database_restored: bool,
    executor: &dyn CommandExecutor,
) -> Result<()> {
    let _lock = OperationLock::acquire(root)?;
    let (current_config, current_secrets, mut live_state) = load_installation(root)?;
    if confirmation != live_state.installation_id {
        bail!("restore confirmation does not match this installation ID");
    }
    if live_state.operation.is_some() && live_state.lifecycle != Lifecycle::RestoreRequired {
        bail!("another incomplete operation must be resolved before restore");
    }
    secure_fs::validate_backup_name(backup_name)?;
    let backup_root = current_config.backup_directory.join(backup_name);
    let manifest = backup::verify_backup(&backup_root, Some(&live_state.installation_id))?;
    let restored_config: InstallationConfig =
        secure_fs::read_json(&backup_root.join("config.json"))?;
    if restored_config.installation_root != root {
        bail!("backup was created for a different installation root");
    }
    let restored_secrets: SecretConfig = secure_fs::read_json(&backup_root.join("secrets.json"))?;
    restored_secrets.validate_for(&restored_config)?;
    let mut restored_state: DeploymentState =
        secure_fs::read_json(&backup_root.join("state.json"))?;
    restored_state.validate(root)?;
    if restored_state.installation_id != live_state.installation_id {
        bail!("backup state belongs to a different installation");
    }
    match (&restored_config.database, &manifest.database) {
        (DatabaseConfig::Bundled { .. }, BackupDatabase::BundledPostgres { major_version: 16 }) => {
        }
        (DatabaseConfig::External { .. }, BackupDatabase::ExternalOperatorSupplied) => {
            if !external_database_restored {
                bail!(
                    "external database restore must be completed by its operator first; then repeat with --external-database-restored"
                );
            }
        }
        _ => bail!("backup database mode is incompatible with its saved configuration"),
    }
    validate_custom_certificate_files(&restored_config)?;

    let runtime = detect_docker(executor, docker_path)?;
    let current_project = ComposeProject::new(
        runtime.executable.clone(),
        root.to_path_buf(),
        &current_config,
    );
    current_project.stop_application_services(executor, &current_secrets.redaction_values())?;
    let recovery = root
        .join("recovery")
        .join(format!("pre-restore-{}", crate::state::now_unix()));
    secure_fs::create_new_secure_directory(&recovery)?;
    secure_fs::copy_protected_file(
        &root.join("config.json"),
        &recovery.join("config.json"),
        secure_fs::MAX_STATE_FILE_BYTES,
    )?;
    secure_fs::copy_protected_file(
        &root.join("secrets.json"),
        &recovery.join("secrets.json"),
        secure_fs::MAX_STATE_FILE_BYTES,
    )?;

    if live_state.lifecycle == Lifecycle::RestoreRequired {
        live_state.operation = None;
    }
    live_state.begin_restore(backup_name.to_string())?;
    live_state.save(&DeploymentState::state_path(root))?;
    persist_active_configuration(root, &restored_config, &restored_secrets)?;
    compose::materialize_assets(root)?;
    restore_local_certificate_if_present(root, &backup_root, restored_config.edge.mode)?;
    compose::ensure_local_certificate(root, &restored_config)?;
    let restored_version = restored_state
        .current
        .as_ref()
        .context("backup state has no deployed version")?;
    compose::materialize_environment(
        root,
        &restored_config,
        &restored_secrets,
        &restored_version.traefik,
    )?;
    let restored_project = ComposeProject::new(
        runtime.executable.clone(),
        root.to_path_buf(),
        &restored_config,
    );

    restored_state.operation = None;
    restored_state.begin_restore(backup_name.to_string())?;
    restored_state.save(&DeploymentState::state_path(root))?;
    let restore_result = (|| -> Result<()> {
        if let DatabaseConfig::Bundled { user, database } = &restored_config.database {
            restored_project.up_database(executor, &restored_secrets.redaction_values())?;
            restored_project.verify_postgres_dump(
                executor,
                &backup_root.join("database.dump"),
                &restored_secrets.redaction_values(),
            )?;
            restored_project.restore_postgres_dump(
                executor,
                user,
                database,
                &backup_root.join("database.dump"),
                &restored_secrets.redaction_values(),
            )?;
        }
        if restored_config.edge.mode == EdgeMode::PublicAcme && manifest.edge_state_included {
            let acme = backup_root.join("acme.json");
            if !acme.exists() {
                bail!("public ACME backup is missing acme.json");
            }
            restored_project.copy_to_acme_volume(
                executor,
                &acme,
                &restored_secrets.redaction_values(),
            )?;
        }
        restored_project.run_database_job(
            executor,
            "database_preflight",
            &restored_secrets.redaction_values(),
        )?;
        restored_state.checkpoint(Checkpoint::MigrationStarted)?;
        restored_state.save(&DeploymentState::state_path(root))?;
        restored_project.run_database_job(
            executor,
            "database_migrate",
            &restored_secrets.redaction_values(),
        )?;
        restored_state.checkpoint(Checkpoint::MigrationCompleted)?;
        restored_state.save(&DeploymentState::state_path(root))?;
        start_and_verify(
            &restored_project,
            executor,
            &restored_config,
            &restored_secrets,
        )?;
        if restored_config.edge.mode == EdgeMode::PublicAcme && manifest.edge_state_included {
            restored_project.restart_traefik(executor, &restored_secrets.redaction_values())?;
        }
        Ok(())
    })();
    if let Err(error) = restore_result {
        restored_state.mark_restore_required();
        restored_state.save(&DeploymentState::state_path(root))?;
        return Err(error).context(format!(
            "restore did not complete; current pre-restore configuration was retained at {}",
            recovery.display()
        ));
    }
    restored_state.promote_healthy()?;
    restored_state.last_verified_backup = Some(backup_name.to_string());
    restored_state.save(&DeploymentState::state_path(root))?;
    println!("Backup {backup_name} was restored and Talos is healthy.");
    Ok(())
}

fn diagnostics_command(
    root: &Path,
    docker_path: Option<&Path>,
    name: Option<&str>,
    executor: &dyn CommandExecutor,
) -> Result<()> {
    let (config, secrets, state) = load_installation(root)?;
    let runtime = detect_docker(executor, docker_path)?;
    let project = ComposeProject::new(runtime.executable.clone(), root.to_path_buf(), &config);
    let path = diagnostics::create_diagnostics(
        name, &config, &secrets, &state, &runtime, &project, executor,
    )?;
    println!("Redacted diagnostics written to {}", path.display());
    Ok(())
}

fn uninstall(
    root: &Path,
    docker_path: Option<&Path>,
    remove_data: bool,
    confirmation: Option<&str>,
    executor: &dyn CommandExecutor,
) -> Result<()> {
    let lock = OperationLock::acquire(root)?;
    let (config, secrets, mut state) = load_installation(root)?;
    require_no_incomplete_operation(&state)?;
    let runtime = detect_docker(executor, docker_path)?;
    let project = ComposeProject::new(runtime.executable, root.to_path_buf(), &config);
    if remove_data {
        if confirmation != Some(state.installation_id.as_str()) {
            bail!("data-removal confirmation does not match this installation ID");
        }
        validate_removal_root(root, &state.installation_id)?;
        project.remove_with_volumes(executor, &secrets.redaction_values())?;
        drop(lock);
        fs::remove_dir_all(root)
            .with_context(|| format!("could not remove installation data {}", root.display()))?;
        println!(
            "Talos containers, named volumes, and installation state were removed. This data is not recoverable unless an off-host backup exists. Remove the talos-server binary through the host package manager."
        );
    } else {
        project.stop(executor, &secrets.redaction_values())?;
        state.lifecycle = Lifecycle::UninstalledDataPreserved;
        state.save(&DeploymentState::state_path(root))?;
        println!(
            "Talos containers were stopped. Durable volumes and protected state at {} were preserved. Remove the talos-server binary through the host package manager.",
            root.display()
        );
    }
    Ok(())
}

fn load_installation(root: &Path) -> Result<(InstallationConfig, SecretConfig, DeploymentState)> {
    secure_fs::reject_symlinks_in_existing_path(root)?;
    let state = DeploymentState::load(&DeploymentState::state_path(root), root)
        .context("Talos is not installed at the selected state directory")?;
    let config: InstallationConfig = secure_fs::read_json(&root.join("config.json"))?;
    let secrets: SecretConfig = secure_fs::read_json(&root.join("secrets.json"))?;
    secrets.validate_for(&config)?;
    let recorded = &state.pending_version()?.config;
    if &config != recorded {
        bail!("active configuration does not match the durable deployment journal");
    }
    Ok((config, secrets, state))
}

fn persist_active_configuration(
    root: &Path,
    config: &InstallationConfig,
    secrets: &SecretConfig,
) -> Result<()> {
    secure_fs::atomic_write_json(&root.join("config.json"), config)?;
    secure_fs::atomic_write_json(&root.join("secrets.json"), secrets)
}

fn persist_rollback_snapshot(
    root: &Path,
    config: &InstallationConfig,
    secrets: &SecretConfig,
) -> Result<()> {
    let rollback = root.join("rollback");
    secure_fs::ensure_secure_directory(&rollback)?;
    secure_fs::atomic_write_json(&rollback.join("config.json"), config)?;
    secure_fs::atomic_write_json(&rollback.join("secrets.json"), secrets)
}

fn recover_interrupted_update(
    root: &Path,
    docker_path: Option<&Path>,
    executor: &dyn CommandExecutor,
    candidate_secrets: &SecretConfig,
    state: &mut DeploymentState,
) -> Result<()> {
    let operation = state
        .operation
        .as_ref()
        .context("incomplete operation has no journal")?;
    if operation.kind != OperationKind::Update {
        bail!("an incomplete non-update operation requires explicit recovery");
    }
    let runtime = detect_docker(executor, docker_path)?;
    recover_failed_update(root, &runtime, executor, candidate_secrets, state).map(|_| ())
}

fn recover_failed_update(
    root: &Path,
    runtime: &DockerRuntime,
    executor: &dyn CommandExecutor,
    candidate_secrets: &SecretConfig,
    state: &mut DeploymentState,
) -> Result<String> {
    let decision = state.rollback_decision()?;
    match decision {
        RollbackDecision::RestorePreviousConfigurationAndRestart
        | RollbackDecision::RollBackTraefikOnly => {
            let previous = state
                .previous_good
                .clone()
                .context("update recovery has no previous known-good version")?;
            let previous_config: InstallationConfig =
                secure_fs::read_json(&root.join("rollback").join("config.json"))?;
            if previous_config != previous.config {
                bail!("rollback snapshot does not match the previous known-good configuration");
            }
            let previous_secrets: SecretConfig =
                secure_fs::read_json(&root.join("rollback").join("secrets.json"))?;
            previous_secrets.validate_for(&previous_config)?;
            persist_active_configuration(root, &previous_config, &previous_secrets)?;
            compose::materialize_assets(root)?;
            compose::ensure_local_certificate(root, &previous_config)?;
            compose::materialize_environment(
                root,
                &previous_config,
                &previous_secrets,
                &previous.traefik,
            )?;
            let project = ComposeProject::new(
                runtime.executable.clone(),
                root.to_path_buf(),
                &previous_config,
            );
            start_and_verify(&project, executor, &previous_config, &previous_secrets)?;
            state.restore_previous_version()?;
            state.save(&DeploymentState::state_path(root))?;
            Ok("the previous known-good configuration and image digests were restored".to_string())
        }
        RollbackDecision::StopAndRestoreVerifiedBackup => {
            let candidate = state.pending_version()?.config.clone();
            let project =
                ComposeProject::new(runtime.executable.clone(), root.to_path_buf(), &candidate);
            let _ =
                project.stop_application_services(executor, &candidate_secrets.redaction_values());
            state.mark_restore_required();
            state.save(&DeploymentState::state_path(root))?;
            Ok("schema migration may have started; application services were stopped and the recorded verified backup must be restored".to_string())
        }
    }
}

fn advance_checkpoint(
    state: &mut DeploymentState,
    state_path: &Path,
    checkpoint: Checkpoint,
) -> Result<()> {
    let current = state
        .operation
        .as_ref()
        .context("operation journal is missing")?
        .checkpoint;
    if checkpoint > current {
        state.checkpoint(checkpoint)?;
        state.save(state_path)?;
    }
    Ok(())
}

fn run_with_guaranteed_cleanup<T, O, C>(operation: O, cleanup: C, cleanup_label: &str) -> Result<T>
where
    O: FnOnce() -> Result<T>,
    C: FnOnce() -> Result<()>,
{
    let operation_result = operation();
    let cleanup_result = cleanup();
    match (operation_result, cleanup_result) {
        (Ok(value), Ok(())) => Ok(value),
        (Ok(_), Err(cleanup_error)) => Err(cleanup_error).context(cleanup_label.to_string()),
        (Err(operation_error), Ok(())) => Err(operation_error),
        (Err(operation_error), Err(cleanup_error)) => {
            Err(operation_error).context(format!("{cleanup_label} also failed: {cleanup_error:#}"))
        }
    }
}

fn start_and_verify(
    project: &ComposeProject,
    executor: &dyn CommandExecutor,
    config: &InstallationConfig,
    secrets: &SecretConfig,
) -> Result<()> {
    project.up_all(executor, &secrets.redaction_values())?;
    let statuses =
        diagnostics::collect_service_status(project, executor, &secrets.redaction_values())?;
    let mut required = vec![
        "api_backend",
        "frontend",
        "talos_relay",
        "talos_server",
        "traefik",
    ];
    if matches!(config.database, DatabaseConfig::Bundled { .. }) {
        required.push("postgres");
    }
    for service in required {
        let status = statuses
            .iter()
            .find(|status| status.service == service)
            .with_context(|| format!("required service {service} is absent from Compose status"))?;
        if status.state != "running"
            || status
                .health
                .as_deref()
                .is_some_and(|health| health != "healthy")
        {
            bail!(
                "required service {service} is not ready (state={}, health={})",
                status.state,
                status.health.as_deref().unwrap_or("not-reported")
            );
        }
    }
    Ok(())
}

fn require_no_incomplete_operation(state: &DeploymentState) -> Result<()> {
    if let Some(operation) = &state.operation {
        bail!(
            "an incomplete {:?} operation is recorded at {:?}; use the corresponding recovery command",
            operation.kind,
            operation.checkpoint
        );
    }
    Ok(())
}

fn ensure_database_identity_unchanged(
    current: &DatabaseConfig,
    candidate: &DatabaseConfig,
) -> Result<()> {
    if current != candidate {
        bail!(
            "update cannot change database mode or identity; migrate the database through a separately reviewed procedure"
        );
    }
    Ok(())
}

fn validate_custom_certificate_files(config: &InstallationConfig) -> Result<()> {
    if config.edge.mode != EdgeMode::CustomCertificate {
        return Ok(());
    }
    let certificate = config
        .edge
        .certificate_path
        .as_deref()
        .context("custom certificate path is missing")?;
    let key = config
        .edge
        .private_key_path
        .as_deref()
        .context("custom private-key path is missing")?;
    secure_fs::reject_symlinks_in_existing_path(certificate)?;
    let certificate_metadata = fs::symlink_metadata(certificate)
        .with_context(|| format!("could not inspect certificate {}", certificate.display()))?;
    if !certificate_metadata.is_file() || certificate_metadata.file_type().is_symlink() {
        bail!("custom certificate must be a regular non-symlink file");
    }
    secure_fs::read_protected_file(key, 4 * 1024 * 1024)
        .context("custom private key must be a protected regular file")?;
    Ok(())
}

fn restore_local_certificate_if_present(
    root: &Path,
    backup_root: &Path,
    edge_mode: EdgeMode,
) -> Result<()> {
    if edge_mode != EdgeMode::Local {
        return Ok(());
    }
    let directory = root.join("local-tls");
    secure_fs::ensure_secure_directory(&directory)?;
    secure_fs::copy_protected_file(
        &backup_root.join("local-certificate.pem"),
        &directory.join("certificate.pem"),
        1024 * 1024,
    )?;
    secure_fs::copy_protected_file(
        &backup_root.join("local-private-key.pem"),
        &directory.join("private-key.pem"),
        1024 * 1024,
    )?;
    secure_fs::copy_protected_file(
        &backup_root.join("local-certificate-metadata.json"),
        &directory.join("metadata.json"),
        1024 * 1024,
    )
}

fn validate_removal_root(root: &Path, installation_id: &str) -> Result<()> {
    let component_count = root
        .components()
        .filter(|component| matches!(component, std::path::Component::Normal(_)))
        .count();
    if !root.is_absolute() || component_count < 2 {
        bail!("refusing to remove an unsafe or overly broad state directory");
    }
    let state = DeploymentState::load(&DeploymentState::state_path(root), root)?;
    if state.installation_id != installation_id {
        bail!("state-directory marker does not match the confirmed installation");
    }
    if let Ok(executable) = std::env::current_exe() {
        if executable.starts_with(root) {
            bail!("refusing to remove a state directory containing the running launcher binary");
        }
    }
    reject_nested_symlinks(root)?;
    Ok(())
}

fn reject_nested_symlinks(path: &Path) -> Result<()> {
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let metadata = entry.file_type()?;
        if metadata.is_symlink() {
            bail!(
                "refusing data removal because {} is a symlink",
                entry.path().display()
            );
        }
        if metadata.is_dir() {
            reject_nested_symlinks(&entry.path())?;
        }
    }
    Ok(())
}

fn frontend_url(config: &InstallationConfig) -> String {
    if config.edge.https_port == 443 {
        format!("https://{}", config.edge.frontend_domain)
    } else {
        format!(
            "https://{}:{}",
            config.edge.frontend_domain, config.edge.https_port
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{OperationJournal, ResolvedImage, STATE_SCHEMA_VERSION};
    use std::{cell::Cell, path::PathBuf};
    use tempfile::TempDir;

    #[test]
    fn removal_guard_rejects_broad_and_mismatched_roots() {
        assert!(validate_removal_root(Path::new("/"), "anything").is_err());
        let temporary = TempDir::new().expect("tempdir");
        let root = temporary.path().join("state");
        secure_fs::ensure_secure_directory(&root).expect("root");
        let mut state = DeploymentState {
            schema_version: STATE_SCHEMA_VERSION,
            installation_id: "a".repeat(32),
            lifecycle: Lifecycle::Stopped,
            current: None,
            previous_good: None,
            operation: None,
            last_verified_backup: None,
            last_transition_unix: 1,
        };
        state
            .save(&DeploymentState::state_path(&root))
            .expect("state");
        assert!(validate_removal_root(&root, &"b".repeat(32)).is_err());
    }

    #[test]
    fn operation_guard_requires_explicit_recovery() {
        let state = DeploymentState {
            schema_version: 1,
            installation_id: "a".repeat(32),
            lifecycle: Lifecycle::Installing,
            current: Some(DeploymentVersion {
                config: dummy_config(),
                traefik: ResolvedImage {
                    digest: format!("traefik@sha256:{}", "a".repeat(64)),
                    version: "v3".to_string(),
                    resolved_at_unix: 1,
                },
            }),
            previous_good: None,
            operation: Some(OperationJournal {
                kind: OperationKind::Update,
                checkpoint: Checkpoint::MigrationStarted,
                started_at_unix: 1,
                backup_name: Some("backup-1".to_string()),
                application_images_changed: true,
            }),
            last_verified_backup: None,
            last_transition_unix: 1,
        };
        assert!(require_no_incomplete_operation(&state).is_err());
    }

    #[test]
    fn guaranteed_cleanup_runs_after_operation_and_cleanup_failures_are_preserved() {
        let cleanup_called = Cell::new(false);
        let error = run_with_guaranteed_cleanup::<(), _, _>(
            || bail!("backup failed"),
            || {
                cleanup_called.set(true);
                bail!("stop failed")
            },
            "temporary database cleanup",
        )
        .expect_err("operation must fail");
        assert!(cleanup_called.get());
        let detail = format!("{error:#}");
        assert!(detail.contains("backup failed"));
        assert!(detail.contains("stop failed"));
    }

    #[test]
    fn update_database_guard_allows_password_rotation_but_rejects_endpoint_changes() {
        use crate::config::ExternalDatabaseIdentity;

        let current = DatabaseConfig::External {
            identity: ExternalDatabaseIdentity::from_url(
                "postgresql://talos:old-password@db.example.com/talos?sslmode=require&connect_timeout=5",
            )
            .expect("current identity"),
        };
        let rotated = DatabaseConfig::External {
            identity: ExternalDatabaseIdentity::from_url(
                "postgresql://talos:new-password@DB.EXAMPLE.COM:5432/talos?sslmode=verify-full&connect_timeout=10",
            )
            .expect("rotated identity"),
        };
        ensure_database_identity_unchanged(&current, &rotated)
            .expect("password rotation must be allowed");

        let changed = DatabaseConfig::External {
            identity: ExternalDatabaseIdentity::from_url(
                "postgresql://talos:new-password@other.example.com/talos?sslmode=verify-full&connect_timeout=10",
            )
            .expect("changed identity"),
        };
        assert!(ensure_database_identity_unchanged(&current, &changed).is_err());
    }

    fn dummy_config() -> InstallationConfig {
        use crate::config::{ExternalDatabaseIdentity, ReleaseImages, CONFIG_SCHEMA_VERSION};
        InstallationConfig {
            schema_version: CONFIG_SCHEMA_VERSION,
            release_version: "1".to_string(),
            update_channel: "stable".to_string(),
            installation_root: PathBuf::from("/tmp/talos-state"),
            backup_directory: PathBuf::from("/tmp/talos-backups"),
            images: ReleaseImages {
                api_backend: format!("api@sha256:{}", "a".repeat(64)),
                frontend: format!("frontend@sha256:{}", "b".repeat(64)),
                relay: format!("relay@sha256:{}", "c".repeat(64)),
                control_server: format!("server@sha256:{}", "d".repeat(64)),
            },
            database: DatabaseConfig::External {
                identity: ExternalDatabaseIdentity::from_url(
                    "postgresql://talos:password@db.example.com/talos?sslmode=verify-full&connect_timeout=5",
                )
                .expect("external database identity"),
            },
            edge: crate::config::EdgeConfig {
                mode: EdgeMode::Local,
                frontend_domain: "talos.localhost".to_string(),
                api_domain: "api.talos.localhost".to_string(),
                control_domain: "control.talos.localhost".to_string(),
                relay_domain: "relay.talos.localhost".to_string(),
                acme_email: None,
                acme_ca_server: None,
                certificate_path: None,
                private_key_path: None,
                http_port: 80,
                https_port: 443,
                subnet: "172.31.240.0/24".to_string(),
                proxy_ipv4: "172.31.240.2".to_string(),
            },
        }
    }
}
