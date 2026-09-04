use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{bail, Context, Result};
use rcgen::{CertificateParams, KeyPair};
use serde::{Deserialize, Serialize};
use time::{Duration as TimeDuration, OffsetDateTime};

use crate::{
    config::{DatabaseConfig, EdgeMode, InstallationConfig, SecretConfig},
    process::{require_success, CommandExecutor, CommandSpec, ProcessOutput},
    secure_fs,
    state::ResolvedImage,
};

const COMPOSE_BASE: &str = include_str!("../../../infra/compose.community.yml");
const COMPOSE_POSTGRES: &str = include_str!("../../../infra/compose.community-postgres.yml");
const COMPOSE_TRAEFIK_ACME: &str = include_str!("../../../infra/compose.community-traefik.yml");
const COMPOSE_TRAEFIK_CUSTOM: &str =
    include_str!("../../../infra/compose.community-traefik-custom.yml");
const COMPOSE_TRAEFIK_LOCAL: &str =
    include_str!("../../../infra/compose.community-traefik-local.yml");
const DYNAMIC_ACME: &str = include_str!("../../../infra/traefik/dynamic-acme.yml");
const DYNAMIC_CUSTOM: &str = include_str!("../../../infra/traefik/dynamic-custom.yml");

const PROJECT_NAME: &str = "talos-community";
const COMPOSE_TIMEOUT: Duration = Duration::from_secs(240);
const MAINTENANCE_TIMEOUT: Duration = Duration::from_secs(900);
const ACME_VOLUME_NAME: &str = "talos-community_talos_traefik_acme";
const ACME_VOLUME_HELPER_IMAGE: &str =
    "postgres:16-alpine@sha256:cf78e76683b9ca8c5733cbbdce6c9262b45b6767934dd0a95e671f9a0fc20685";
const ACME_RESTORE_TEMP_PATH: &str = "/acme/.talos-server-acme.restore";

#[derive(Clone, Debug)]
pub struct ComposeProject {
    docker: PathBuf,
    root: PathBuf,
    files: Vec<PathBuf>,
}

impl ComposeProject {
    pub fn new(docker: PathBuf, root: PathBuf, config: &InstallationConfig) -> Self {
        let compose_root = root.join("compose");
        let mut files = vec![compose_root.join("compose.community.yml")];
        if matches!(config.database, DatabaseConfig::Bundled { .. }) {
            files.push(compose_root.join("compose.community-postgres.yml"));
        }
        files.push(compose_root.join(match config.edge.mode {
            EdgeMode::PublicAcme => "compose.community-traefik.yml",
            EdgeMode::CustomCertificate => "compose.community-traefik-custom.yml",
            EdgeMode::Local => "compose.community-traefik-local.yml",
        }));
        Self {
            docker,
            root,
            files,
        }
    }

    pub fn env_path(&self) -> PathBuf {
        self.root.join("talos.env")
    }

    pub fn files(&self) -> &[PathBuf] {
        &self.files
    }

    pub fn base_command(&self) -> CommandSpec {
        let mut command = CommandSpec::new(&self.docker).args([
            OsString::from("compose"),
            OsString::from("--project-name"),
            OsString::from(PROJECT_NAME),
            OsString::from("--env-file"),
            self.env_path().into_os_string(),
        ]);
        for file in &self.files {
            command = command.arg("-f").arg(file.as_os_str());
        }
        command
    }

    pub fn validate(&self, executor: &dyn CommandExecutor, secrets: &[&str]) -> Result<()> {
        let output = executor.execute(
            &self.base_command().args(["config", "--quiet"]),
            COMPOSE_TIMEOUT,
        )?;
        require_success("Compose configuration validation", &output, secrets)
    }

    pub fn up_database(&self, executor: &dyn CommandExecutor, secrets: &[&str]) -> Result<()> {
        let output = executor.execute(
            &self.base_command().args([
                "up",
                "--detach",
                "--wait",
                "--wait-timeout",
                "180",
                "postgres",
            ]),
            COMPOSE_TIMEOUT,
        )?;
        require_success("bundled PostgreSQL readiness", &output, secrets)
    }

    pub fn run_database_job(
        &self,
        executor: &dyn CommandExecutor,
        service: &str,
        secrets: &[&str],
    ) -> Result<()> {
        if !matches!(service, "database_preflight" | "database_migrate") {
            bail!("unsupported database job");
        }
        let output = executor.execute(
            &self
                .base_command()
                .args(["run", "--rm", "--no-deps", service]),
            MAINTENANCE_TIMEOUT,
        )?;
        require_success(service, &output, secrets)
    }

    pub fn up_all(&self, executor: &dyn CommandExecutor, secrets: &[&str]) -> Result<()> {
        let output = executor.execute(
            &self
                .base_command()
                .args(["up", "--detach", "--wait", "--wait-timeout", "180"]),
            COMPOSE_TIMEOUT,
        )?;
        require_success("Talos service readiness", &output, secrets)
    }

    pub fn stop(&self, executor: &dyn CommandExecutor, secrets: &[&str]) -> Result<()> {
        let output = executor.execute(
            &self.base_command().args(["down", "--timeout", "60"]),
            COMPOSE_TIMEOUT,
        )?;
        require_success("Talos stop", &output, secrets)
    }

    pub fn stop_application_services(
        &self,
        executor: &dyn CommandExecutor,
        secrets: &[&str],
    ) -> Result<()> {
        let output = executor.execute(
            &self.base_command().args([
                "stop",
                "frontend",
                "api_backend",
                "talos_server",
                "talos_relay",
                "traefik",
            ]),
            COMPOSE_TIMEOUT,
        )?;
        require_success("Talos application stop", &output, secrets)
    }

    pub fn stop_database(&self, executor: &dyn CommandExecutor, secrets: &[&str]) -> Result<()> {
        let output = executor.execute(
            &self.base_command().args(["stop", "postgres"]),
            COMPOSE_TIMEOUT,
        )?;
        require_success("bundled PostgreSQL stop", &output, secrets)
    }

    pub fn restart_traefik(&self, executor: &dyn CommandExecutor, secrets: &[&str]) -> Result<()> {
        let output = executor.execute(
            &self.base_command().args(["restart", "traefik"]),
            COMPOSE_TIMEOUT,
        )?;
        require_success("Traefik restart", &output, secrets)
    }

    pub fn stop_traefik(&self, executor: &dyn CommandExecutor, secrets: &[&str]) -> Result<()> {
        let output = executor.execute(
            &self.base_command().args(["stop", "traefik"]),
            COMPOSE_TIMEOUT,
        )?;
        require_success("Traefik backup quiesce", &output, secrets)
    }

    pub fn remove_with_volumes(
        &self,
        executor: &dyn CommandExecutor,
        secrets: &[&str],
    ) -> Result<()> {
        let output = executor.execute(
            &self
                .base_command()
                .args(["down", "--timeout", "60", "--volumes", "--remove-orphans"]),
            COMPOSE_TIMEOUT,
        )?;
        require_success("Talos data removal", &output, secrets)
    }

    pub fn ps(&self, executor: &dyn CommandExecutor) -> Result<ProcessOutput> {
        executor.execute(
            &self.base_command().args(["ps", "--format", "json"]),
            COMPOSE_TIMEOUT,
        )
    }

    pub fn logs(&self, executor: &dyn CommandExecutor) -> Result<ProcessOutput> {
        executor.execute(
            &self
                .base_command()
                .args(["logs", "--no-color", "--tail", "200"]),
            COMPOSE_TIMEOUT,
        )
    }

    pub fn postgres_dump(
        &self,
        executor: &dyn CommandExecutor,
        user: &str,
        database: &str,
        output_path: &Path,
        secrets: &[&str],
    ) -> Result<()> {
        let output = executor.execute_to_file(
            &self.base_command().args([
                "exec",
                "--no-TTY",
                "postgres",
                "pg_dump",
                "--format=custom",
                "--no-owner",
                "--username",
                user,
                "--dbname",
                database,
            ]),
            MAINTENANCE_TIMEOUT,
            output_path,
        )?;
        require_success("PostgreSQL logical backup", &output, secrets)
    }

    pub fn verify_postgres_dump(
        &self,
        executor: &dyn CommandExecutor,
        dump_path: &Path,
        secrets: &[&str],
    ) -> Result<()> {
        let output = executor.execute_with_input(
            &self
                .base_command()
                .args(["exec", "--no-TTY", "postgres", "pg_restore", "--list"]),
            MAINTENANCE_TIMEOUT,
            dump_path,
        )?;
        require_success("PostgreSQL backup verification", &output, secrets)
    }

    pub fn restore_postgres_dump(
        &self,
        executor: &dyn CommandExecutor,
        user: &str,
        database: &str,
        dump_path: &Path,
        secrets: &[&str],
    ) -> Result<()> {
        for command in [
            vec![
                "exec",
                "--no-TTY",
                "postgres",
                "dropdb",
                "--if-exists",
                "--force",
                "--username",
                user,
                database,
            ],
            vec![
                "exec",
                "--no-TTY",
                "postgres",
                "createdb",
                "--username",
                user,
                database,
            ],
        ] {
            let output =
                executor.execute(&self.base_command().args(command), MAINTENANCE_TIMEOUT)?;
            require_success("PostgreSQL restore preparation", &output, secrets)?;
        }
        let output = executor.execute_with_input(
            &self.base_command().args([
                "exec",
                "--no-TTY",
                "postgres",
                "pg_restore",
                "--exit-on-error",
                "--no-owner",
                "--username",
                user,
                "--dbname",
                database,
            ]),
            MAINTENANCE_TIMEOUT,
            dump_path,
        )?;
        require_success("PostgreSQL logical restore", &output, secrets)
    }

    pub fn copy_from_acme_volume(
        &self,
        executor: &dyn CommandExecutor,
        output_path: &Path,
        secrets: &[&str],
    ) -> Result<()> {
        if output_path.exists() {
            bail!(
                "refusing to overwrite ACME backup output {}",
                output_path.display()
            );
        }
        self.ensure_acme_helper_image(executor, secrets)?;
        let inspect = executor.execute(
            &CommandSpec::new(&self.docker).args(["volume", "inspect", ACME_VOLUME_NAME]),
            COMPOSE_TIMEOUT,
        )?;
        require_success("Traefik ACME volume inspection", &inspect, secrets)?;

        let container = self.create_acme_transfer_container(executor, secrets)?;
        let source = format!("{container}:/acme/acme.json");
        let copy_result = executor
            .execute(
                &CommandSpec::new(&self.docker)
                    .arg("cp")
                    .arg(source)
                    .arg(output_path.as_os_str()),
                COMPOSE_TIMEOUT,
            )
            .and_then(|output| require_success("Traefik ACME backup", &output, secrets));
        let cleanup_result = self.remove_helper_container(executor, &container, secrets);
        combine_operation_and_cleanup(copy_result, cleanup_result, "ACME backup helper cleanup")
    }

    pub fn copy_to_acme_volume(
        &self,
        executor: &dyn CommandExecutor,
        input_path: &Path,
        secrets: &[&str],
    ) -> Result<()> {
        secure_fs::read_protected_file(input_path, secure_fs::MAX_STATE_FILE_BYTES)
            .context("ACME restore input must be a protected regular file")?;
        self.ensure_acme_helper_image(executor, secrets)?;
        let create_volume = executor.execute(
            &CommandSpec::new(&self.docker).args([
                "volume",
                "create",
                "--label",
                "com.docker.compose.project=talos-community",
                "--label",
                "com.docker.compose.volume=talos_traefik_acme",
                ACME_VOLUME_NAME,
            ]),
            COMPOSE_TIMEOUT,
        )?;
        require_success("Traefik ACME volume creation", &create_volume, secrets)?;

        self.run_acme_volume_command(
            executor,
            "/bin/rm",
            &["-f", ACME_RESTORE_TEMP_PATH],
            secrets,
        )?;
        let container = self.create_acme_transfer_container(executor, secrets)?;
        let destination = format!("{container}:{ACME_RESTORE_TEMP_PATH}");
        let copy_result = executor
            .execute(
                &CommandSpec::new(&self.docker)
                    .arg("cp")
                    .arg(input_path.as_os_str())
                    .arg(destination),
                COMPOSE_TIMEOUT,
            )
            .and_then(|output| require_success("Traefik ACME restore staging", &output, secrets));
        let cleanup_result = self.remove_helper_container(executor, &container, secrets);
        combine_operation_and_cleanup(copy_result, cleanup_result, "ACME restore helper cleanup")?;

        self.run_acme_volume_command(
            executor,
            "/bin/chown",
            &["0:0", ACME_RESTORE_TEMP_PATH],
            secrets,
        )?;
        self.run_acme_volume_command(
            executor,
            "/bin/chmod",
            &["600", ACME_RESTORE_TEMP_PATH],
            secrets,
        )?;
        self.run_acme_volume_command(
            executor,
            "/bin/mv",
            &["-f", ACME_RESTORE_TEMP_PATH, "/acme/acme.json"],
            secrets,
        )?;
        let mode = self.run_acme_volume_command(
            executor,
            "/bin/stat",
            &["-c", "%a", "/acme/acme.json"],
            secrets,
        )?;
        if crate::process::output_text(&mode.stdout) != "600" {
            bail!("restored Traefik ACME state does not have mode 0600");
        }
        Ok(())
    }

    fn ensure_acme_helper_image(
        &self,
        executor: &dyn CommandExecutor,
        secrets: &[&str],
    ) -> Result<()> {
        let inspect = executor.execute(
            &CommandSpec::new(&self.docker).args(["image", "inspect", ACME_VOLUME_HELPER_IMAGE]),
            COMPOSE_TIMEOUT,
        )?;
        if inspect.status.success() {
            return Ok(());
        }
        let pull = executor.execute(
            &CommandSpec::new(&self.docker).args(["pull", ACME_VOLUME_HELPER_IMAGE]),
            MAINTENANCE_TIMEOUT,
        )?;
        require_success("immutable ACME volume helper image pull", &pull, secrets)
    }

    fn create_acme_transfer_container(
        &self,
        executor: &dyn CommandExecutor,
        secrets: &[&str],
    ) -> Result<String> {
        let output = executor.execute(
            &self
                .acme_helper_base("create")
                .arg("--entrypoint")
                .arg("/bin/true")
                .arg(ACME_VOLUME_HELPER_IMAGE),
            COMPOSE_TIMEOUT,
        )?;
        require_success("ACME transfer helper creation", &output, secrets)?;
        let container = crate::process::output_text(&output.stdout);
        if !(12..=64).contains(&container.len())
            || !container.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            bail!("Docker returned a malformed ACME transfer helper identifier");
        }
        Ok(container)
    }

    fn remove_helper_container(
        &self,
        executor: &dyn CommandExecutor,
        container: &str,
        secrets: &[&str],
    ) -> Result<()> {
        let output = executor.execute(
            &CommandSpec::new(&self.docker).args(["rm", "--force", container]),
            COMPOSE_TIMEOUT,
        )?;
        require_success("ACME transfer helper removal", &output, secrets)
    }

    fn run_acme_volume_command(
        &self,
        executor: &dyn CommandExecutor,
        entrypoint: &str,
        arguments: &[&str],
        secrets: &[&str],
    ) -> Result<ProcessOutput> {
        let output = executor.execute(
            &self
                .acme_helper_base("run")
                .arg("--rm")
                .arg("--entrypoint")
                .arg(entrypoint)
                .arg(ACME_VOLUME_HELPER_IMAGE)
                .args(arguments.iter().copied()),
            COMPOSE_TIMEOUT,
        )?;
        require_success("ACME volume operation", &output, secrets)?;
        Ok(output)
    }

    fn acme_helper_base(&self, operation: &str) -> CommandSpec {
        CommandSpec::new(&self.docker).args([
            operation,
            "--network",
            "none",
            "--read-only",
            "--cap-drop",
            "ALL",
            "--cap-add",
            "CHOWN",
            "--security-opt",
            "no-new-privileges:true",
            "--mount",
            "type=volume,source=talos-community_talos_traefik_acme,target=/acme",
        ])
    }
}

pub(crate) fn combine_operation_and_cleanup<T>(
    operation: Result<T>,
    cleanup: Result<()>,
    cleanup_label: &str,
) -> Result<T> {
    match (operation, cleanup) {
        (Ok(value), Ok(())) => Ok(value),
        (Ok(_), Err(cleanup_error)) => Err(cleanup_error).context(cleanup_label.to_string()),
        (Err(operation_error), Ok(())) => Err(operation_error),
        (Err(operation_error), Err(cleanup_error)) => {
            Err(operation_error).context(format!("{cleanup_label} also failed: {cleanup_error:#}"))
        }
    }
}

pub fn materialize_assets(root: &Path) -> Result<()> {
    let compose_root = root.join("compose");
    let dynamic_root = compose_root.join("traefik");
    secure_fs::ensure_secure_directory(&dynamic_root)?;
    for (relative, contents) in [
        ("compose.community.yml", COMPOSE_BASE),
        ("compose.community-postgres.yml", COMPOSE_POSTGRES),
        ("compose.community-traefik.yml", COMPOSE_TRAEFIK_ACME),
        (
            "compose.community-traefik-custom.yml",
            COMPOSE_TRAEFIK_CUSTOM,
        ),
        ("compose.community-traefik-local.yml", COMPOSE_TRAEFIK_LOCAL),
        ("traefik/dynamic-acme.yml", DYNAMIC_ACME),
        ("traefik/dynamic-custom.yml", DYNAMIC_CUSTOM),
    ] {
        secure_fs::atomic_write(&compose_root.join(relative), contents.as_bytes())?;
    }
    Ok(())
}

pub fn materialize_environment(
    root: &Path,
    config: &InstallationConfig,
    secrets: &SecretConfig,
    traefik: &ResolvedImage,
) -> Result<()> {
    config.images.validate()?;
    secrets.validate_for(config)?;
    crate::config::validate_digest_image("Traefik", &traefik.digest)?;

    let mut variables = vec![
        (
            "TALOS_API_BACKEND_IMAGE",
            config.images.api_backend.as_str(),
        ),
        ("TALOS_FRONTEND_IMAGE", config.images.frontend.as_str()),
        ("TALOS_RELAY_IMAGE", config.images.relay.as_str()),
        ("TALOS_SERVER_IMAGE", config.images.control_server.as_str()),
        ("TALOS_JWT_SECRET", secrets.jwt_secret.as_str()),
        (
            "TALOS_APP_ENCRYPTION_KEY",
            secrets.app_encryption_key.as_str(),
        ),
        (
            "TALOS_RMM_SERVER_API_KEY",
            secrets.rmm_server_api_key.as_str(),
        ),
        (
            "TALOS_FRONTEND_DOMAIN",
            config.edge.frontend_domain.as_str(),
        ),
        ("TALOS_API_DOMAIN", config.edge.api_domain.as_str()),
        ("TALOS_CONTROL_DOMAIN", config.edge.control_domain.as_str()),
        ("TALOS_RELAY_DOMAIN", config.edge.relay_domain.as_str()),
        ("TALOS_EDGE_SUBNET", config.edge.subnet.as_str()),
        ("TALOS_TRAEFIK_IPV4", config.edge.proxy_ipv4.as_str()),
        ("TALOS_TRAEFIK_IMAGE", traefik.digest.as_str()),
    ];
    let frontend_url = public_https_url(&config.edge.frontend_domain, config.edge.https_port);
    let api_url = public_https_url(&config.edge.api_domain, config.edge.https_port);
    let control_url = public_https_url(&config.edge.control_domain, config.edge.https_port);
    let agent_url = format!("{}/agent/ws", control_url.replacen("https://", "wss://", 1));
    let relay_address = format!("{}:{}", config.edge.relay_domain, config.edge.https_port);
    let http_port = config.edge.http_port.to_string();
    let https_port = config.edge.https_port.to_string();
    variables.extend([
        ("TALOS_PUBLIC_FRONTEND_URL", frontend_url.as_str()),
        ("TALOS_PUBLIC_API_URL", api_url.as_str()),
        ("TALOS_PUBLIC_RMM_API_URL", control_url.as_str()),
        ("TALOS_AGENT_SERVER_URL", agent_url.as_str()),
        ("TALOS_PUBLIC_RELAY_ADDRESS", relay_address.as_str()),
        ("TALOS_EDGE_HTTP_PORT", http_port.as_str()),
        ("TALOS_EDGE_HTTPS_PORT", https_port.as_str()),
    ]);

    let mut owned_variables: Vec<(String, String)> = Vec::new();
    match &config.database {
        DatabaseConfig::Bundled { user, database } => {
            variables.push(("TALOS_POSTGRES_USER", user));
            variables.push(("TALOS_POSTGRES_DATABASE", database));
            variables.push((
                "TALOS_POSTGRES_PASSWORD",
                secrets
                    .postgres_password
                    .as_deref()
                    .context("bundled PostgreSQL password is missing")?,
            ));
        }
        DatabaseConfig::External { .. } => variables.push((
            "TALOS_DATABASE_URL",
            secrets
                .external_database_url
                .as_deref()
                .context("external PostgreSQL URL is missing")?,
        )),
    }
    match config.edge.mode {
        EdgeMode::PublicAcme => {
            variables.push((
                "TALOS_ACME_EMAIL",
                config
                    .edge
                    .acme_email
                    .as_deref()
                    .context("ACME email is missing")?,
            ));
            if let Some(server) = config.edge.acme_ca_server.as_deref() {
                variables.push(("TALOS_ACME_CA_SERVER", server));
            }
        }
        EdgeMode::CustomCertificate => {
            owned_variables.push((
                "TALOS_CUSTOM_TLS_CERT_PATH".to_string(),
                path_text(
                    config
                        .edge
                        .certificate_path
                        .as_deref()
                        .context("custom certificate path is missing")?,
                )?,
            ));
            owned_variables.push((
                "TALOS_CUSTOM_TLS_KEY_PATH".to_string(),
                path_text(
                    config
                        .edge
                        .private_key_path
                        .as_deref()
                        .context("custom private-key path is missing")?,
                )?,
            ));
        }
        EdgeMode::Local => {
            owned_variables.push((
                "TALOS_LOCAL_TLS_CERT_PATH".to_string(),
                path_text(&root.join("local-tls").join("certificate.pem"))?,
            ));
            owned_variables.push((
                "TALOS_LOCAL_TLS_KEY_PATH".to_string(),
                path_text(&root.join("local-tls").join("private-key.pem"))?,
            ));
        }
    }

    let mut output = String::from("# Generated by talos-server. Contains secrets; do not share.\n");
    for (key, value) in variables {
        output.push_str(key);
        output.push('=');
        output.push_str(&dotenv_quote(value)?);
        output.push('\n');
    }
    for (key, value) in owned_variables {
        output.push_str(&key);
        output.push('=');
        output.push_str(&dotenv_quote(&value)?);
        output.push('\n');
    }
    secure_fs::atomic_write(&root.join("talos.env"), output.as_bytes())
}

pub fn ensure_local_certificate(root: &Path, config: &InstallationConfig) -> Result<()> {
    if config.edge.mode != EdgeMode::Local {
        return Ok(());
    }
    let directory = root.join("local-tls");
    secure_fs::ensure_secure_directory(&directory)?;
    let certificate_path = directory.join("certificate.pem");
    let key_path = directory.join("private-key.pem");
    let metadata_path = directory.join("metadata.json");
    match (
        certificate_path.exists(),
        key_path.exists(),
        metadata_path.exists(),
    ) {
        (true, true, true) => {
            let certificate = secure_fs::read_protected_file(&certificate_path, 1024 * 1024)?;
            let key = secure_fs::read_protected_file(&key_path, 1024 * 1024)?;
            let metadata: LocalCertificateMetadata = secure_fs::read_json(&metadata_path)?;
            if !certificate
                .windows(b"-----BEGIN CERTIFICATE-----".len())
                .any(|window| window == b"-----BEGIN CERTIFICATE-----")
                || !key
                    .windows(PRIVATE_KEY_MARKER.len())
                    .any(|window| window == PRIVATE_KEY_MARKER.as_bytes())
                || metadata.schema_version != 1
            {
                bail!("generated local TLS material is malformed");
            }
            if metadata.not_after_unix > crate::state::now_unix() + 3 * 24 * 60 * 60 {
                return Ok(());
            }
        }
        (false, false, false) => {}
        _ => {
            bail!("local TLS certificate state is incomplete; preserve the existing file and repair from backup")
        }
    }

    let now = OffsetDateTime::now_utc();
    let mut parameters = CertificateParams::new(vec![
        config.edge.frontend_domain.clone(),
        config.edge.api_domain.clone(),
        config.edge.control_domain.clone(),
        config.edge.relay_domain.clone(),
    ])
    .context("could not generate local self-signed TLS certificate")?;
    parameters.not_before = now - TimeDuration::minutes(5);
    parameters.not_after = now + TimeDuration::days(30);
    let signing_key = KeyPair::generate().context("could not generate local TLS private key")?;
    let cert = parameters
        .self_signed(&signing_key)
        .context("could not self-sign local TLS certificate")?;
    secure_fs::atomic_write(&certificate_path, cert.pem().as_bytes())?;
    secure_fs::atomic_write(&key_path, signing_key.serialize_pem().as_bytes())?;
    secure_fs::atomic_write_json(
        &metadata_path,
        &LocalCertificateMetadata {
            schema_version: 1,
            not_after_unix: (now + TimeDuration::days(30)).unix_timestamp() as u64,
        },
    )?;
    Ok(())
}

const PRIVATE_KEY_MARKER: &str = concat!("-----BEGIN PRIVATE ", "KEY-----");

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LocalCertificateMetadata {
    schema_version: u32,
    not_after_unix: u64,
}

fn public_https_url(domain: &str, port: u16) -> String {
    if port == 443 {
        format!("https://{domain}")
    } else {
        format!("https://{domain}:{port}")
    }
}

fn path_text(path: &Path) -> Result<String> {
    path.to_str()
        .map(ToString::to_string)
        .context("configured path is not valid Unicode")
}

fn dotenv_quote(value: &str) -> Result<String> {
    if value.chars().any(char::is_control) || value.contains('\'') {
        bail!("configuration value cannot be represented safely in the Compose environment file");
    }
    Ok(format!("'{value}'"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{
        EdgeConfig, ExternalDatabaseIdentity, ReleaseImages, CONFIG_SCHEMA_VERSION,
    };
    use std::{cell::RefCell, process::ExitStatus};
    use tempfile::TempDir;

    struct RecordingExecutor {
        commands: RefCell<Vec<CommandSpec>>,
        fail_copy: bool,
        fail_entrypoint: Option<&'static str>,
    }

    impl RecordingExecutor {
        fn successful() -> Self {
            Self {
                commands: RefCell::new(Vec::new()),
                fail_copy: false,
                fail_entrypoint: None,
            }
        }

        fn output_for(&self, spec: &CommandSpec) -> ProcessOutput {
            let arguments: Vec<_> = spec
                .args
                .iter()
                .map(|argument| argument.to_string_lossy())
                .collect();
            let entrypoint = arguments
                .iter()
                .position(|argument| argument == "--entrypoint")
                .and_then(|index| arguments.get(index + 1))
                .map(|argument| argument.as_ref());
            let failed = (self.fail_copy && arguments.first().is_some_and(|value| value == "cp"))
                || self
                    .fail_entrypoint
                    .is_some_and(|expected| entrypoint == Some(expected));
            let stdout = if arguments.first().is_some_and(|value| value == "create") {
                format!("{}\n", "a".repeat(64)).into_bytes()
            } else if entrypoint == Some("/bin/stat") {
                b"600\n".to_vec()
            } else {
                Vec::new()
            };
            ProcessOutput {
                status: test_exit_status(!failed),
                stdout,
                stderr: if failed {
                    b"simulated helper failure".to_vec()
                } else {
                    Vec::new()
                },
                stdout_truncated: false,
                stderr_truncated: false,
            }
        }
    }

    impl CommandExecutor for RecordingExecutor {
        fn execute(&self, spec: &CommandSpec, _timeout: Duration) -> Result<ProcessOutput> {
            self.commands.borrow_mut().push(spec.clone());
            Ok(self.output_for(spec))
        }

        fn execute_to_file(
            &self,
            _spec: &CommandSpec,
            _timeout: Duration,
            _output: &Path,
        ) -> Result<ProcessOutput> {
            bail!("unexpected execute_to_file in ACME volume test")
        }

        fn execute_with_input(
            &self,
            _spec: &CommandSpec,
            _timeout: Duration,
            _input: &Path,
        ) -> Result<ProcessOutput> {
            bail!("unexpected execute_with_input in ACME volume test")
        }
    }

    #[cfg(unix)]
    fn test_exit_status(success: bool) -> ExitStatus {
        use std::os::unix::process::ExitStatusExt;
        ExitStatus::from_raw(if success { 0 } else { 1 << 8 })
    }

    #[cfg(windows)]
    fn test_exit_status(success: bool) -> ExitStatus {
        use std::os::windows::process::ExitStatusExt;
        ExitStatus::from_raw(if success { 0 } else { 1 })
    }

    fn digest(name: &str, byte: char) -> String {
        format!(
            "registry.example/{name}@sha256:{}",
            byte.to_string().repeat(64)
        )
    }

    fn config(root: &Path, mode: EdgeMode, database: DatabaseConfig) -> InstallationConfig {
        InstallationConfig {
            schema_version: CONFIG_SCHEMA_VERSION,
            release_version: "1.0.0".to_string(),
            update_channel: "stable".to_string(),
            installation_root: root.to_path_buf(),
            backup_directory: root.join("backups"),
            images: ReleaseImages {
                api_backend: digest("api", 'a'),
                frontend: digest("frontend", 'b'),
                relay: digest("relay", 'c'),
                control_server: digest("server", 'd'),
            },
            database,
            edge: EdgeConfig {
                mode,
                frontend_domain: "talos.example.com".to_string(),
                api_domain: "api.talos.example.com".to_string(),
                control_domain: "control.talos.example.com".to_string(),
                relay_domain: "relay.talos.example.com".to_string(),
                acme_email: (mode == EdgeMode::PublicAcme).then(|| "admin@example.com".to_string()),
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

    fn external_database() -> DatabaseConfig {
        DatabaseConfig::External {
            identity: ExternalDatabaseIdentity::from_url(
                "postgresql://talos:encoded%21@db.example.com/talos?sslmode=verify-full&connect_timeout=5",
            )
            .expect("external database identity"),
        }
    }

    #[test]
    fn selects_exact_database_and_edge_layers() {
        let root = PathBuf::from("/var/lib/talos-server");
        let bundled = config(
            &root,
            EdgeMode::PublicAcme,
            DatabaseConfig::Bundled {
                user: "talos".to_string(),
                database: "talos".to_string(),
            },
        );
        let project = ComposeProject::new(PathBuf::from("/usr/bin/docker"), root, &bundled);
        let names: Vec<_> = project
            .files()
            .iter()
            .filter_map(|path| path.file_name().and_then(|name| name.to_str()))
            .collect();
        assert_eq!(
            names,
            [
                "compose.community.yml",
                "compose.community-postgres.yml",
                "compose.community-traefik.yml"
            ]
        );
    }

    #[test]
    fn embedded_contract_has_no_source_build_or_docker_socket() {
        assert!(!COMPOSE_BASE.contains("build:"));
        assert!(!COMPOSE_BASE.contains("ports:"));
        for source in [
            COMPOSE_TRAEFIK_ACME,
            COMPOSE_TRAEFIK_CUSTOM,
            COMPOSE_TRAEFIK_LOCAL,
        ] {
            assert!(!source.contains("docker.sock"));
            assert!(source.contains("TALOS_TRAEFIK_IMAGE"));
            assert!(source.contains("API_TRUSTED_PROXIES"));
        }
        assert!(COMPOSE_TRAEFIK_ACME.contains(&format!("name: {ACME_VOLUME_NAME}")));
        assert!(ACME_VOLUME_HELPER_IMAGE.contains("@sha256:"));
    }

    #[test]
    fn environment_contains_digest_images_and_not_a_tag_only_traefik_reference() {
        let temporary = TempDir::new().expect("tempdir");
        let root = temporary.path().join("state");
        let config = config(&root, EdgeMode::PublicAcme, external_database());
        let secrets = SecretConfig {
            schema_version: 1,
            jwt_secret: "1".repeat(64),
            app_encryption_key: "2".repeat(64),
            rmm_server_api_key: "3".repeat(64),
            postgres_password: None,
            external_database_url: Some("postgresql://talos:encoded%21@db.example.com/talos?sslmode=verify-full&connect_timeout=5".to_string()),
        };
        let traefik = ResolvedImage {
            digest: digest("traefik", 'e'),
            version: "v3.5.0".to_string(),
            resolved_at_unix: 1,
        };
        materialize_environment(&root, &config, &secrets, &traefik).expect("environment");
        let environment =
            secure_fs::read_protected_file(&root.join("talos.env"), 1024 * 1024).expect("read env");
        let environment = String::from_utf8(environment).expect("utf8");
        assert!(environment.contains(&format!("TALOS_TRAEFIK_IMAGE='{}'", traefik.digest)));
        assert!(!environment.contains("traefik:latest"));
    }

    #[test]
    fn local_certificate_is_generated_once_with_restrictive_storage() {
        let temporary = TempDir::new().expect("tempdir");
        let root = temporary.path().join("state");
        let config = config(&root, EdgeMode::Local, external_database());
        ensure_local_certificate(&root, &config).expect("certificate");
        let first = secure_fs::read_protected_file(
            &root.join("local-tls").join("private-key.pem"),
            1024 * 1024,
        )
        .expect("key");
        ensure_local_certificate(&root, &config).expect("idempotent");
        let second = secure_fs::read_protected_file(
            &root.join("local-tls").join("private-key.pem"),
            1024 * 1024,
        )
        .expect("key");
        assert_eq!(first, second);
    }

    #[test]
    fn acme_backup_uses_only_the_named_volume_and_removes_helper_after_copy_failure() {
        let temporary = TempDir::new().expect("tempdir");
        let root = temporary.path().join("state");
        let project = ComposeProject::new(
            PathBuf::from("/usr/bin/docker"),
            root,
            &config(temporary.path(), EdgeMode::PublicAcme, external_database()),
        );
        let executor = RecordingExecutor {
            commands: RefCell::new(Vec::new()),
            fail_copy: true,
            fail_entrypoint: None,
        };
        let result =
            project.copy_from_acme_volume(&executor, &temporary.path().join("acme.json"), &[]);
        assert!(result.is_err());

        let commands = executor.commands.borrow();
        let create = commands
            .iter()
            .find(|command| command.args.first() == Some(&OsString::from("create")))
            .expect("transfer helper create command");
        let create_arguments: Vec<_> = create
            .args
            .iter()
            .map(|argument| argument.to_string_lossy())
            .collect();
        assert!(create_arguments
            .windows(2)
            .any(|pair| pair == ["--network", "none"]));
        assert!(create_arguments
            .windows(2)
            .any(|pair| pair == ["--cap-drop", "ALL"]));
        assert!(create_arguments
            .windows(2)
            .any(|pair| pair == ["--cap-add", "CHOWN"]));
        assert!(create_arguments
            .windows(2)
            .any(|pair| pair == ["--security-opt", "no-new-privileges:true"]));
        assert_eq!(
            create_arguments
                .iter()
                .filter(|argument| argument.as_ref() == "--mount")
                .count(),
            1
        );
        assert!(create_arguments.iter().any(|argument| {
            argument.as_ref()
                == "type=volume,source=talos-community_talos_traefik_acme,target=/acme"
        }));
        assert!(!commands.iter().any(|command| {
            command
                .args
                .iter()
                .any(|argument| argument.to_string_lossy().contains("traefik:/acme"))
        }));
        assert!(commands.iter().any(|command| {
            command.args
                == [
                    OsString::from("rm"),
                    OsString::from("--force"),
                    OsString::from("a".repeat(64)),
                ]
        }));
    }

    #[test]
    fn acme_restore_uses_direct_entrypoints_and_auto_removes_failing_mutation_helpers() {
        for failing_entrypoint in ["/bin/chmod", "/bin/mv"] {
            let temporary = TempDir::new().expect("tempdir");
            let root = temporary.path().join("state");
            let input = temporary.path().join("acme.json");
            secure_fs::atomic_write(&input, b"protected ACME state").expect("protected input");
            let project = ComposeProject::new(
                PathBuf::from("/usr/bin/docker"),
                root,
                &config(temporary.path(), EdgeMode::PublicAcme, external_database()),
            );
            let executor = RecordingExecutor {
                commands: RefCell::new(Vec::new()),
                fail_copy: false,
                fail_entrypoint: Some(failing_entrypoint),
            };
            assert!(project.copy_to_acme_volume(&executor, &input, &[]).is_err());

            let commands = executor.commands.borrow();
            assert!(!commands.iter().any(|command| {
                command
                    .args
                    .iter()
                    .any(|argument| matches!(argument.to_str(), Some("/bin/sh" | "sh" | "-c")))
            }));
            let failing = commands
                .iter()
                .find(|command| {
                    command.args.windows(2).any(|pair| {
                        pair == [
                            OsString::from("--entrypoint"),
                            OsString::from(failing_entrypoint),
                        ]
                    })
                })
                .expect("failing direct helper command");
            assert_eq!(failing.args.first(), Some(&OsString::from("run")));
            assert!(failing.args.contains(&OsString::from("--rm")));
            assert!(commands.iter().any(|command| {
                command.args
                    == [
                        OsString::from("rm"),
                        OsString::from("--force"),
                        OsString::from("a".repeat(64)),
                    ]
            }));
        }
    }

    #[test]
    fn acme_restore_verifies_mode_after_atomic_rename() {
        let temporary = TempDir::new().expect("tempdir");
        let input = temporary.path().join("acme.json");
        secure_fs::atomic_write(&input, b"protected ACME state").expect("protected input");
        let project = ComposeProject::new(
            PathBuf::from("/usr/bin/docker"),
            temporary.path().join("state"),
            &config(temporary.path(), EdgeMode::PublicAcme, external_database()),
        );
        let executor = RecordingExecutor::successful();
        project
            .copy_to_acme_volume(&executor, &input, &[])
            .expect("restore command contract");
        let commands = executor.commands.borrow();
        let entrypoints: Vec<_> = commands
            .iter()
            .filter_map(|command| {
                command
                    .args
                    .iter()
                    .position(|argument| argument == "--entrypoint")
                    .and_then(|index| command.args.get(index + 1))
                    .and_then(|value| value.to_str())
            })
            .collect();
        assert_eq!(
            entrypoints,
            [
                "/bin/rm",
                "/bin/true",
                "/bin/chown",
                "/bin/chmod",
                "/bin/mv",
                "/bin/stat"
            ]
        );
    }
}
