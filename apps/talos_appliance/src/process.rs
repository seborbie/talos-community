use std::{
    ffi::OsString,
    fs::{self, File, OpenOptions},
    io::Read,
    path::{Path, PathBuf},
    process::{Command, ExitStatus, Stdio},
    thread,
    time::{Duration, Instant},
};

use anyhow::{bail, Context, Result};

#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;

const OUTPUT_LIMIT: usize = 1024 * 1024;
const POLL_INTERVAL: Duration = Duration::from_millis(50);
const INHERITED_HOST_ENVIRONMENT: &[&str] = &["SYSTEMROOT", "TEMP", "TMP", "WINDIR"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandSpec {
    pub program: PathBuf,
    pub args: Vec<OsString>,
    pub environment: Vec<(OsString, OsString)>,
}

impl CommandSpec {
    pub fn new(program: impl Into<PathBuf>) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
            environment: Vec::new(),
        }
    }

    pub fn arg(mut self, value: impl Into<OsString>) -> Self {
        self.args.push(value.into());
        self
    }

    pub fn args<I, S>(mut self, values: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<OsString>,
    {
        self.args.extend(values.into_iter().map(Into::into));
        self
    }

    pub fn env(mut self, key: impl Into<OsString>, value: impl Into<OsString>) -> Self {
        self.environment.push((key.into(), value.into()));
        self
    }
}

#[derive(Debug)]
pub struct ProcessOutput {
    pub status: ExitStatus,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
}

pub trait CommandExecutor {
    fn execute(&self, spec: &CommandSpec, timeout: Duration) -> Result<ProcessOutput>;
    fn execute_to_file(
        &self,
        spec: &CommandSpec,
        timeout: Duration,
        output: &Path,
    ) -> Result<ProcessOutput>;
    fn execute_with_input(
        &self,
        spec: &CommandSpec,
        timeout: Duration,
        input: &Path,
    ) -> Result<ProcessOutput>;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct SystemExecutor;

impl CommandExecutor for SystemExecutor {
    fn execute(&self, spec: &CommandSpec, timeout: Duration) -> Result<ProcessOutput> {
        execute_process(spec, timeout, Stdio::null(), Stdio::piped(), true)
    }

    fn execute_to_file(
        &self,
        spec: &CommandSpec,
        timeout: Duration,
        output: &Path,
    ) -> Result<ProcessOutput> {
        if output.exists() {
            bail!("refusing to overwrite process output {}", output.display());
        }
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        options.mode(0o600);
        let file = options
            .open(output)
            .with_context(|| format!("could not create process output {}", output.display()))?;
        if let Err(error) = crate::secure_fs::harden_regular_file(output) {
            drop(file);
            let _ = fs::remove_file(output);
            return Err(error).context("could not protect process output before writing");
        }
        let result = execute_process(spec, timeout, Stdio::null(), Stdio::from(file), false);
        if result.is_err() || result.as_ref().is_ok_and(|output| !output.status.success()) {
            let _ = fs::remove_file(output);
        }
        result
    }

    fn execute_with_input(
        &self,
        spec: &CommandSpec,
        timeout: Duration,
        input: &Path,
    ) -> Result<ProcessOutput> {
        let file = File::open(input)
            .with_context(|| format!("could not open process input {}", input.display()))?;
        execute_process(spec, timeout, Stdio::from(file), Stdio::piped(), true)
    }
}

fn execute_process(
    spec: &CommandSpec,
    timeout: Duration,
    stdin: Stdio,
    stdout: Stdio,
    capture_stdout: bool,
) -> Result<ProcessOutput> {
    if !spec.program.is_absolute() {
        bail!("child executable path must be absolute");
    }
    let mut command = Command::new(&spec.program);
    command
        .args(&spec.args)
        .env_clear()
        .stdin(stdin)
        .stdout(stdout)
        .stderr(Stdio::piped());
    for (key, value) in child_environment(spec, |key| std::env::var_os(key))? {
        command.env(key, value);
    }

    let mut child = command
        .spawn()
        .with_context(|| format!("could not start {}", spec.program.display()))?;
    let stdout_reader = if capture_stdout {
        child.stdout.take().map(spawn_bounded_reader)
    } else {
        None
    };
    let stderr_reader = child.stderr.take().map(spawn_bounded_reader);
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().context("could not poll child process")? {
            break status;
        }
        if started.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            bail!(
                "{} exceeded the {} second operation timeout and was terminated",
                spec.program.display(),
                timeout.as_secs()
            );
        }
        thread::sleep(POLL_INTERVAL);
    };

    let (stdout, stdout_truncated) = join_reader(stdout_reader)?;
    let (stderr, stderr_truncated) = join_reader(stderr_reader)?;
    Ok(ProcessOutput {
        status,
        stdout,
        stderr,
        stdout_truncated,
        stderr_truncated,
    })
}

fn child_environment<F>(spec: &CommandSpec, mut host_value: F) -> Result<Vec<(OsString, OsString)>>
where
    F: FnMut(&str) -> Option<OsString>,
{
    let mut environment = Vec::new();
    for key in INHERITED_HOST_ENVIRONMENT {
        if let Some(value) = host_value(key) {
            environment.push((OsString::from(key), value));
        }
    }
    for (key, value) in &spec.environment {
        if is_reserved_routing_environment(key) {
            bail!(
                "child environment variable {} is reserved by the launcher security boundary",
                key.to_string_lossy()
            );
        }
        environment.push((key.clone(), value.clone()));
    }
    Ok(environment)
}

fn is_reserved_routing_environment(key: &std::ffi::OsStr) -> bool {
    let normalized = key.to_string_lossy().to_ascii_uppercase();
    normalized == "HOME"
        || normalized == "PATH"
        || normalized == "USERPROFILE"
        || normalized == "XDG_CONFIG_HOME"
        || normalized.starts_with("DOCKER_")
        || normalized.starts_with("COMPOSE_")
}

fn spawn_bounded_reader<R>(mut reader: R) -> thread::JoinHandle<Result<(Vec<u8>, bool)>>
where
    R: Read + Send + 'static,
{
    thread::spawn(move || {
        let mut captured = Vec::new();
        let mut truncated = false;
        let mut buffer = [0_u8; 8192];
        loop {
            let read = reader
                .read(&mut buffer)
                .context("could not read child output")?;
            if read == 0 {
                break;
            }
            let remaining = OUTPUT_LIMIT.saturating_sub(captured.len());
            captured.extend_from_slice(&buffer[..read.min(remaining)]);
            if read > remaining {
                truncated = true;
            }
        }
        Ok((captured, truncated))
    })
}

fn join_reader(
    reader: Option<thread::JoinHandle<Result<(Vec<u8>, bool)>>>,
) -> Result<(Vec<u8>, bool)> {
    match reader {
        Some(reader) => reader
            .join()
            .map_err(|_| anyhow::anyhow!("child output reader panicked"))?,
        None => Ok((Vec::new(), false)),
    }
}

pub fn locate_executable(explicit: Option<&Path>, executable_name: &str) -> Result<PathBuf> {
    if let Some(path) = explicit {
        if !path.is_absolute() {
            bail!("explicit {executable_name} path must be absolute");
        }
        return canonical_executable(path, executable_name);
    }

    for candidate in known_candidates(executable_name) {
        if candidate.is_file() {
            return canonical_executable(&candidate, executable_name);
        }
    }
    if let Some(path_value) = std::env::var_os("PATH") {
        for directory in std::env::split_paths(&path_value) {
            if !directory.is_absolute() {
                continue;
            }
            let candidate = directory.join(platform_executable_name(executable_name));
            if candidate.is_file() {
                return canonical_executable(&candidate, executable_name);
            }
        }
    }
    bail!(
        "{executable_name} was not found; install a supported container runtime manually and ensure its CLI is available"
    )
}

fn canonical_executable(path: &Path, label: &str) -> Result<PathBuf> {
    let canonical = path
        .canonicalize()
        .with_context(|| format!("could not resolve {label} executable {}", path.display()))?;
    if !canonical.is_file() {
        bail!(
            "{label} executable {} is not a regular file",
            canonical.display()
        );
    }
    Ok(canonical)
}

fn platform_executable_name(name: &str) -> OsString {
    #[cfg(windows)]
    {
        OsString::from(format!("{name}.exe"))
    }
    #[cfg(not(windows))]
    {
        OsString::from(name)
    }
}

fn known_candidates(name: &str) -> Vec<PathBuf> {
    #[cfg(windows)]
    {
        let executable = platform_executable_name(name);
        let mut paths = Vec::new();
        for root in [
            std::env::var_os("ProgramFiles"),
            std::env::var_os("ProgramW6432"),
        ]
        .into_iter()
        .flatten()
        {
            paths.push(
                PathBuf::from(root)
                    .join("Docker")
                    .join("Docker")
                    .join("resources")
                    .join("bin")
                    .join(&executable),
            );
        }
        paths
    }
    #[cfg(not(windows))]
    {
        vec![
            PathBuf::from(format!("/usr/bin/{name}")),
            PathBuf::from(format!("/usr/local/bin/{name}")),
            PathBuf::from(format!("/opt/homebrew/bin/{name}")),
        ]
    }
}

pub fn output_text(output: &[u8]) -> String {
    String::from_utf8_lossy(output).trim().to_string()
}

pub fn require_success(label: &str, output: &ProcessOutput, secrets: &[&str]) -> Result<()> {
    if output.status.success() {
        return Ok(());
    }
    let detail = crate::redaction::redact_text(&output_text(&output.stderr), secrets);
    if detail.is_empty() {
        bail!("{label} failed with status {}", output.status);
    }
    bail!("{label} failed: {detail}");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_spec_keeps_hostile_values_as_one_argv_element() {
        let spec = CommandSpec::new(PathBuf::from("/usr/bin/docker"))
            .arg("compose")
            .arg("--project-name")
            .arg("talos; touch /tmp/pwned")
            .arg("$(whoami)");
        assert_eq!(spec.args.len(), 4);
        assert_eq!(spec.args[2], OsString::from("talos; touch /tmp/pwned"));
        assert_eq!(spec.args[3], OsString::from("$(whoami)"));
    }

    #[test]
    fn child_environment_excludes_caller_controlled_docker_routing_and_credentials() {
        let spec =
            CommandSpec::new(PathBuf::from("/usr/bin/docker")).env("TALOS_OPERATION", "backup");
        let environment =
            child_environment(&spec, |key| Some(OsString::from(format!("host-{key}"))))
                .expect("environment");

        assert_eq!(
            environment,
            vec![
                (
                    OsString::from("SYSTEMROOT"),
                    OsString::from("host-SYSTEMROOT")
                ),
                (OsString::from("TEMP"), OsString::from("host-TEMP")),
                (OsString::from("TMP"), OsString::from("host-TMP")),
                (OsString::from("WINDIR"), OsString::from("host-WINDIR")),
                (OsString::from("TALOS_OPERATION"), OsString::from("backup")),
            ]
        );
        for forbidden in [
            "DOCKER_CONFIG",
            "DOCKER_CONTEXT",
            "DOCKER_HOST",
            "DOCKER_CERT_PATH",
            "DOCKER_TLS_VERIFY",
            "COMPOSE_FILE",
            "COMPOSE_ENV_FILES",
            "HOME",
            "PATH",
            "USERPROFILE",
            "XDG_CONFIG_HOME",
        ] {
            assert!(
                environment.iter().all(|(key, _)| key != forbidden),
                "inherited {forbidden}"
            );
        }
    }

    #[test]
    fn command_spec_cannot_reintroduce_reserved_docker_routing_environment() {
        for key in [
            "DOCKER_HOST",
            "docker_context",
            "DOCKER_CONFIG",
            "DOCKER_CERT_PATH",
            "COMPOSE_FILE",
            "HOME",
            "PATH",
            "USERPROFILE",
            "XDG_CONFIG_HOME",
        ] {
            let spec = CommandSpec::new(PathBuf::from("/usr/bin/docker")).env(key, "hostile");
            let error = child_environment(&spec, |_| None).expect_err("reserved key must fail");
            assert!(error.to_string().contains("reserved"), "{key}: {error:#}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn system_executor_child_receives_exact_filtered_environment() {
        let spec = CommandSpec::new(PathBuf::from("/usr/bin/env")).env("TALOS_CHILD_TEST", "exact");
        let mut expected: Vec<String> = child_environment(&spec, |key| std::env::var_os(key))
            .expect("expected child environment")
            .into_iter()
            .map(|(key, value)| format!("{}={}", key.to_string_lossy(), value.to_string_lossy()))
            .collect();
        expected.sort();

        let output = SystemExecutor
            .execute(&spec, Duration::from_secs(5))
            .expect("execute env");
        assert!(output.status.success());
        let mut actual: Vec<String> = String::from_utf8(output.stdout)
            .expect("UTF-8 environment")
            .lines()
            .map(ToString::to_string)
            .collect();
        actual.sort();
        assert_eq!(actual, expected);
    }

    #[test]
    fn refuses_relative_child_programs() {
        let executor = SystemExecutor;
        let error = executor
            .execute(
                &CommandSpec::new("docker").arg("version"),
                Duration::from_secs(1),
            )
            .expect_err("relative executable must fail");
        assert!(error.to_string().contains("absolute"));
    }
}
