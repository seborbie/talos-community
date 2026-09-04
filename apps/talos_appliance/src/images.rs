use std::{path::Path, path::PathBuf, time::Duration};

use anyhow::{bail, Context, Result};

use crate::{
    config::{validate_digest_image, ReleaseImages},
    process::{locate_executable, output_text, require_success, CommandExecutor, CommandSpec},
    state::{now_unix, ResolvedImage},
};

const DOCKER_TIMEOUT: Duration = Duration::from_secs(120);
const IMAGE_PULL_TIMEOUT: Duration = Duration::from_secs(900);

#[derive(Clone, Debug)]
pub struct DockerRuntime {
    pub executable: PathBuf,
    pub compose_version: String,
    pub engine_version: String,
}

pub fn detect_docker(
    executor: &dyn CommandExecutor,
    explicit_path: Option<&Path>,
) -> Result<DockerRuntime> {
    let executable = locate_executable(explicit_path, "docker")?;
    let compose = executor.execute(
        &CommandSpec::new(&executable).args(["compose", "version", "--short"]),
        DOCKER_TIMEOUT,
    )?;
    require_success("Docker Compose v2 prerequisite", &compose, &[])?;
    let compose_version = output_text(&compose.stdout);
    let major = compose_version
        .trim_start_matches('v')
        .split('.')
        .next()
        .and_then(|value| value.parse::<u32>().ok())
        .context("Docker Compose returned an unrecognized version")?;
    if major < 2 {
        bail!("Docker Compose v2 or newer is required");
    }

    let engine = executor.execute(
        &CommandSpec::new(&executable).args(["info", "--format", "{{json .ServerVersion}}"]),
        DOCKER_TIMEOUT,
    )?;
    require_success(
        "Docker Engine prerequisite (the daemon may not be running or accessible)",
        &engine,
        &[],
    )?;
    let engine_version: String = serde_json::from_str(&output_text(&engine.stdout))
        .context("Docker Engine returned an unrecognized version")?;
    if engine_version.trim().is_empty() {
        bail!("Docker Engine returned an empty version");
    }

    Ok(DockerRuntime {
        executable,
        compose_version,
        engine_version,
    })
}

pub fn pull_release_images(
    executor: &dyn CommandExecutor,
    docker: &Path,
    images: &ReleaseImages,
) -> Result<()> {
    images.validate()?;
    for image in images.values() {
        let output = executor.execute(
            &CommandSpec::new(docker).args(["pull", image]),
            IMAGE_PULL_TIMEOUT,
        )?;
        require_success("immutable Talos image pull", &output, &[])?;
    }
    Ok(())
}

pub fn resolve_traefik_latest(
    executor: &dyn CommandExecutor,
    docker: &Path,
) -> Result<ResolvedImage> {
    let pull = executor.execute(
        &CommandSpec::new(docker).args(["pull", "traefik:latest"]),
        IMAGE_PULL_TIMEOUT,
    )?;
    require_success("Traefik latest resolution", &pull, &[])?;

    let inspect = executor.execute(
        &CommandSpec::new(docker).args([
            "image",
            "inspect",
            "traefik:latest",
            "--format",
            "{{json .RepoDigests}}",
        ]),
        DOCKER_TIMEOUT,
    )?;
    require_success("Traefik digest inspection", &inspect, &[])?;
    let digest = parse_traefik_repo_digests(&output_text(&inspect.stdout))?;

    let version_output = executor.execute(
        &CommandSpec::new(docker).args([
            "image",
            "inspect",
            digest.as_str(),
            "--format",
            "{{json (index .Config.Labels \"org.opencontainers.image.version\")}}",
        ]),
        DOCKER_TIMEOUT,
    )?;
    require_success("Traefik version inspection", &version_output, &[])?;
    let version = parse_optional_json_string(&output_text(&version_output.stdout))
        .unwrap_or_else(|| "unknown".to_string());

    Ok(ResolvedImage {
        digest,
        version,
        resolved_at_unix: now_unix(),
    })
}

pub fn ensure_recorded_images(
    executor: &dyn CommandExecutor,
    docker: &Path,
    images: &ReleaseImages,
    traefik: &ResolvedImage,
) -> Result<()> {
    images.validate()?;
    validate_digest_image("Traefik", &traefik.digest)?;
    for image in images
        .values()
        .into_iter()
        .chain(std::iter::once(traefik.digest.as_str()))
    {
        let inspect = executor.execute(
            &CommandSpec::new(docker).args(["image", "inspect", image]),
            DOCKER_TIMEOUT,
        )?;
        if !inspect.status.success() {
            let pull = executor.execute(
                &CommandSpec::new(docker).args(["pull", image]),
                IMAGE_PULL_TIMEOUT,
            )?;
            require_success("recorded immutable image pull", &pull, &[])?;
        }
    }
    Ok(())
}

fn parse_traefik_repo_digests(value: &str) -> Result<String> {
    let digests: Vec<String> =
        serde_json::from_str(value).context("Traefik image did not expose repository digests")?;
    let digest = digests
        .into_iter()
        .find(|candidate| {
            candidate
                .split_once("@sha256:")
                .is_some_and(|(repository, _)| {
                    repository == "traefik"
                        || repository == "docker.io/library/traefik"
                        || repository == "index.docker.io/library/traefik"
                })
        })
        .context("Traefik image did not expose an official repository digest")?;
    validate_digest_image("Traefik", &digest)?;
    Ok(digest)
}

fn parse_optional_json_string(value: &str) -> Option<String> {
    serde_json::from_str::<Option<String>>(value)
        .ok()
        .flatten()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty() && value.len() <= 128)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selects_only_an_official_traefik_digest() {
        let digest = "a".repeat(64);
        let input = format!(
            "[\"docker.io/library/traefik@sha256:{digest}\",\"mirror.invalid/not-traefik@sha256:{}\"]",
            "b".repeat(64)
        );
        assert_eq!(
            parse_traefik_repo_digests(&input).expect("digest"),
            format!("docker.io/library/traefik@sha256:{digest}")
        );
        assert!(parse_traefik_repo_digests(&format!(
            "[\"registry.invalid/lookalike@sha256:{}\"]",
            "b".repeat(64)
        ))
        .is_err());
        assert!(parse_traefik_repo_digests(&format!(
            "[\"registry.invalid/traefik@sha256:{}\"]",
            "b".repeat(64)
        ))
        .is_err());
    }

    #[test]
    fn version_label_parser_fails_closed_to_unknown() {
        assert_eq!(
            parse_optional_json_string("\"v3.5.2\"").as_deref(),
            Some("v3.5.2")
        );
        assert_eq!(parse_optional_json_string("null"), None);
        assert_eq!(parse_optional_json_string("not-json"), None);
    }
}
