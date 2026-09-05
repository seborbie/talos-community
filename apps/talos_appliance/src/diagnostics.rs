use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    compose::ComposeProject,
    config::{EdgeMode, InstallationConfig, SecretConfig},
    images::DockerRuntime,
    process::{output_text, CommandExecutor},
    redaction::redact_text,
    secure_fs,
    state::{now_unix, DeploymentState},
};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ServiceStatus {
    pub service: String,
    pub state: String,
    pub health: Option<String>,
    pub exit_code: Option<i64>,
}

#[derive(Serialize)]
struct DiagnosticsSummary<'a> {
    generated_at_unix: u64,
    installation_id: &'a str,
    lifecycle: String,
    release_version: &'a str,
    talos_images: &'a crate::config::ReleaseImages,
    traefik: &'a crate::state::ResolvedImage,
    docker_compose_version: &'a str,
    docker_engine_version: &'a str,
    edge_mode: EdgeMode,
    routes: [&'a str; 4],
    certificate: CertificateStatus,
    last_verified_backup: &'a Option<String>,
}

#[derive(Serialize)]
struct CertificateStatus {
    owner: &'static str,
    expiry: &'static str,
    note: &'static str,
}

pub fn collect_service_status(
    project: &ComposeProject,
    executor: &dyn CommandExecutor,
    secrets: &[&str],
) -> Result<Vec<ServiceStatus>> {
    let output = project.ps(executor)?;
    if !output.status.success() {
        let detail = redact_text(&output_text(&output.stderr), secrets);
        bail!("could not inspect Talos services: {detail}");
    }
    parse_compose_ps(&output_text(&output.stdout))
}

pub fn create_diagnostics(
    name: Option<&str>,
    config: &InstallationConfig,
    secrets: &SecretConfig,
    state: &DeploymentState,
    runtime: &DockerRuntime,
    project: &ComposeProject,
    executor: &dyn CommandExecutor,
) -> Result<PathBuf> {
    let name = name
        .map(ToString::to_string)
        .unwrap_or_else(|| format!("diagnostics-{}", now_unix()));
    secure_fs::validate_backup_name(&name)?;
    let root = config.backup_directory.join(&name);
    secure_fs::create_new_secure_directory(&root)?;

    let current = state.pending_version()?;
    let certificate = match config.edge.mode {
        EdgeMode::PublicAcme => CertificateStatus {
            owner: "traefik_acme",
            expiry: "not_probed",
            note: "The launcher records ACME ownership and state presence. Endpoint expiry probing remains a release gate.",
        },
        EdgeMode::CustomCertificate => CertificateStatus {
            owner: "operator",
            expiry: "not_probed",
            note: "Certificate expiry must be monitored by the external certificate owner.",
        },
        EdgeMode::Local => CertificateStatus {
            owner: "talos_server_local_self_signed",
            expiry: "not_probed",
            note: "Local mode is loopback-only and uses launcher-generated self-signed material.",
        },
    };
    let summary = DiagnosticsSummary {
        generated_at_unix: now_unix(),
        installation_id: &state.installation_id,
        lifecycle: format!("{:?}", state.lifecycle).to_ascii_lowercase(),
        release_version: &config.release_version,
        talos_images: &config.images,
        traefik: &current.traefik,
        docker_compose_version: &runtime.compose_version,
        docker_engine_version: &runtime.engine_version,
        edge_mode: config.edge.mode,
        routes: [
            &config.edge.frontend_domain,
            &config.edge.api_domain,
            &config.edge.control_domain,
            &config.edge.relay_domain,
        ],
        certificate,
        last_verified_backup: &state.last_verified_backup,
    };
    secure_fs::atomic_write_json(&root.join("summary.json"), &summary)?;

    let statuses = collect_service_status(project, executor, &secrets.redaction_values())?;
    secure_fs::atomic_write_json(&root.join("services.json"), &statuses)?;

    let logs = project.logs(executor)?;
    let mut combined = output_text(&logs.stdout);
    if !logs.stderr.is_empty() {
        combined.push('\n');
        combined.push_str(&output_text(&logs.stderr));
    }
    if logs.stdout_truncated || logs.stderr_truncated {
        combined.push_str("\n[OUTPUT TRUNCATED]\n");
    }
    secure_fs::atomic_write(
        &root.join("recent-logs.txt"),
        redact_text(&combined, &secrets.redaction_values()).as_bytes(),
    )?;

    let disk = collect_disk_capacity(config, executor)
        .unwrap_or_else(|error| format!("disk capacity unavailable: {error:#}"));
    secure_fs::atomic_write(&root.join("disk-capacity.txt"), disk.as_bytes())?;
    Ok(root)
}

fn collect_disk_capacity(
    config: &InstallationConfig,
    executor: &dyn CommandExecutor,
) -> Result<String> {
    #[cfg(not(windows))]
    {
        use crate::process::CommandSpec;
        use std::time::Duration;
        let executable = [PathBuf::from("/bin/df"), PathBuf::from("/usr/bin/df")]
            .into_iter()
            .find(|candidate| candidate.is_file())
            .context("df executable is unavailable")?;
        let output = executor.execute(
            &CommandSpec::new(executable)
                .arg("-Pk")
                .arg(config.installation_root.as_os_str()),
            Duration::from_secs(30),
        )?;
        if !output.status.success() {
            bail!("df returned {}", output.status);
        }
        return Ok(output_text(&output.stdout));
    }
    #[cfg(windows)]
    {
        let _ = (config, executor);
        bail!("Windows disk-capacity collection is not implemented in this release")
    }
}

fn parse_compose_ps(input: &str) -> Result<Vec<ServiceStatus>> {
    if input.trim().is_empty() {
        return Ok(Vec::new());
    }
    let values: Vec<Value> = match serde_json::from_str::<Value>(input) {
        Ok(Value::Array(values)) => values,
        Ok(value @ Value::Object(_)) => vec![value],
        Ok(_) => bail!("Docker Compose ps returned an unexpected JSON value"),
        Err(_) => input
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| {
                serde_json::from_str::<Value>(line)
                    .context("Docker Compose ps returned malformed JSON")
            })
            .collect::<Result<Vec<_>>>()?,
    };
    let mut statuses = Vec::new();
    for value in values {
        let object = value
            .as_object()
            .context("Docker Compose ps entry is not an object")?;
        let service = string_field(object, "Service")?;
        let state = string_field(object, "State")?;
        if !service
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
            || state.len() > 64
        {
            bail!("Docker Compose ps returned an unsafe status value");
        }
        let health = object
            .get("Health")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string);
        let exit_code = object.get("ExitCode").and_then(Value::as_i64);
        statuses.push(ServiceStatus {
            service: service.to_string(),
            state: state.to_string(),
            health,
            exit_code,
        });
    }
    statuses.sort_by(|left, right| left.service.cmp(&right.service));
    Ok(statuses)
}

fn string_field<'a>(object: &'a serde_json::Map<String, Value>, name: &str) -> Result<&'a str> {
    object
        .get(name)
        .and_then(Value::as_str)
        .with_context(|| format!("Docker Compose ps entry is missing {name}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_array_and_line_delimited_compose_status() {
        let array = r#"[{"Service":"frontend","State":"running","Health":"healthy","ExitCode":0}]"#;
        assert_eq!(
            parse_compose_ps(array).expect("array"),
            vec![ServiceStatus {
                service: "frontend".to_string(),
                state: "running".to_string(),
                health: Some("healthy".to_string()),
                exit_code: Some(0),
            }]
        );
        let lines = "{\"Service\":\"frontend\",\"State\":\"running\"}\n{\"Service\":\"api_backend\",\"State\":\"exited\"}";
        assert_eq!(parse_compose_ps(lines).expect("lines").len(), 2);
    }

    #[test]
    fn rejects_status_field_injection() {
        let hostile = r#"[{"Service":"frontend\nJWT_SECRET=oops","State":"running"}]"#;
        assert!(parse_compose_ps(hostile).is_err());
    }
}
