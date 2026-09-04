use std::{ffi::OsString, path::PathBuf};

use anyhow::{bail, Context, Result};

use crate::secure_fs;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cli {
    pub state_dir: PathBuf,
    pub docker_path: Option<PathBuf>,
    pub command: CliCommand,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CliCommand {
    Install {
        config: PathBuf,
        external_database_backup: Option<PathBuf>,
    },
    Start,
    Stop,
    Status,
    Update {
        config: PathBuf,
        external_database_backup: Option<PathBuf>,
    },
    Backup {
        name: Option<String>,
        external_database_backup: Option<PathBuf>,
    },
    Restore {
        backup_name: String,
        confirmation: String,
        external_database_restored: bool,
    },
    Diagnostics {
        name: Option<String>,
    },
    Uninstall {
        remove_data: bool,
        confirmation: Option<String>,
    },
    Help,
}

impl Cli {
    pub fn parse<I>(arguments: I) -> Result<Self>
    where
        I: IntoIterator<Item = OsString>,
    {
        let mut arguments = arguments.into_iter().peekable();
        let mut state_dir = None;
        let mut docker_path = None;
        let command_name = loop {
            let argument = arguments.next().unwrap_or_else(|| OsString::from("help"));
            let text = argument
                .to_str()
                .context("command and option names must be valid Unicode")?;
            match text {
                "--state-dir" => {
                    set_once_path(
                        &mut state_dir,
                        required_value(&mut arguments, "--state-dir")?,
                        "--state-dir",
                    )?;
                }
                "--docker" => {
                    set_once_path(
                        &mut docker_path,
                        required_value(&mut arguments, "--docker")?,
                        "--docker",
                    )?;
                }
                "-h" | "--help" | "help" => break "help".to_string(),
                value if value.starts_with('-') => bail!("unknown global option {value}"),
                value => break value.to_string(),
            }
        };
        let state_dir = state_dir.unwrap_or(secure_fs::default_state_root()?);

        let remaining: Vec<OsString> = arguments.collect();
        let command = parse_command(&command_name, &remaining)?;
        Ok(Self {
            state_dir,
            docker_path,
            command,
        })
    }
}

fn parse_command(name: &str, arguments: &[OsString]) -> Result<CliCommand> {
    match name {
        "help" => {
            require_no_arguments(arguments)?;
            Ok(CliCommand::Help)
        }
        "install" => {
            let mut config = None;
            let mut external_database_backup = None;
            parse_options(arguments, |key, value| match key {
                "--config" => set_once_path(&mut config, required_option_value(key, value)?, key),
                "--external-database-backup" => set_once_path(
                    &mut external_database_backup,
                    required_option_value(key, value)?,
                    key,
                ),
                _ => bail!("unknown install option {key}"),
            })?;
            Ok(CliCommand::Install {
                config: config.context("--config is required")?,
                external_database_backup,
            })
        }
        "start" => {
            require_no_arguments(arguments)?;
            Ok(CliCommand::Start)
        }
        "stop" => {
            require_no_arguments(arguments)?;
            Ok(CliCommand::Stop)
        }
        "status" => {
            require_no_arguments(arguments)?;
            Ok(CliCommand::Status)
        }
        "update" => {
            let mut config = None;
            let mut external_database_backup = None;
            parse_options(arguments, |key, value| match key {
                "--config" => set_once_path(&mut config, required_option_value(key, value)?, key),
                "--external-database-backup" => set_once_path(
                    &mut external_database_backup,
                    required_option_value(key, value)?,
                    key,
                ),
                _ => bail!("unknown update option {key}"),
            })?;
            Ok(CliCommand::Update {
                config: config.context("--config is required")?,
                external_database_backup,
            })
        }
        "backup" => {
            let mut name = None;
            let mut external_database_backup = None;
            parse_options(arguments, |key, value| match key {
                "--name" => {
                    set_once_string(&mut name, required_option_value(key, value)?, key)?;
                    secure_fs::validate_backup_name(name.as_deref().unwrap_or_default())
                }
                "--external-database-backup" => set_once_path(
                    &mut external_database_backup,
                    required_option_value(key, value)?,
                    key,
                ),
                _ => bail!("unknown backup option {key}"),
            })?;
            Ok(CliCommand::Backup {
                name,
                external_database_backup,
            })
        }
        "restore" => {
            let backup_name = arguments
                .first()
                .and_then(|value| value.to_str())
                .context("restore requires one backup name")?
                .to_string();
            secure_fs::validate_backup_name(&backup_name)?;
            let mut confirmation = None;
            let mut external_database_restored = false;
            parse_options(&arguments[1..], |key, value| match key {
                "--confirm" => {
                    set_once_string(&mut confirmation, required_option_value(key, value)?, key)
                }
                "--external-database-restored" => {
                    if value.is_some() || external_database_restored {
                        bail!("--external-database-restored may be specified only once without a value");
                    }
                    external_database_restored = true;
                    Ok(())
                }
                _ => bail!("unknown restore option {key}"),
            })?;
            Ok(CliCommand::Restore {
                backup_name,
                confirmation: confirmation
                    .context("restore requires --confirm <installation-id>")?,
                external_database_restored,
            })
        }
        "diagnostics" => {
            let mut output_name = None;
            parse_options(arguments, |key, value| match key {
                "--name" => {
                    set_once_string(&mut output_name, required_option_value(key, value)?, key)?;
                    secure_fs::validate_backup_name(output_name.as_deref().unwrap_or_default())
                }
                _ => bail!("unknown diagnostics option {key}"),
            })?;
            Ok(CliCommand::Diagnostics { name: output_name })
        }
        "uninstall" => {
            let mut remove_data = false;
            let mut confirmation = None;
            parse_options(arguments, |key, value| match key {
                "--remove-data" => {
                    if value.is_some() || remove_data {
                        bail!("--remove-data may be specified only once without a value");
                    }
                    remove_data = true;
                    Ok(())
                }
                "--confirm" => {
                    set_once_string(&mut confirmation, required_option_value(key, value)?, key)
                }
                _ => bail!("unknown uninstall option {key}"),
            })?;
            if remove_data && confirmation.is_none() {
                bail!("--remove-data requires --confirm <installation-id>");
            }
            if !remove_data && confirmation.is_some() {
                bail!("--confirm is valid only with --remove-data");
            }
            Ok(CliCommand::Uninstall {
                remove_data,
                confirmation,
            })
        }
        other => bail!("unknown command {other}; run talos-server help"),
    }
}

fn parse_options<F>(arguments: &[OsString], mut visitor: F) -> Result<()>
where
    F: FnMut(&str, Option<OsString>) -> Result<()>,
{
    let mut index = 0;
    while index < arguments.len() {
        let key = arguments[index]
            .to_str()
            .context("option names must be valid Unicode")?;
        if !key.starts_with("--") {
            bail!("unexpected positional argument {key}");
        }
        let takes_no_value = matches!(key, "--remove-data" | "--external-database-restored");
        let value = if takes_no_value {
            None
        } else {
            index += 1;
            Some(
                arguments
                    .get(index)
                    .cloned()
                    .with_context(|| format!("{key} requires a value"))?,
            )
        };
        visitor(key, value)?;
        index += 1;
    }
    Ok(())
}

fn required_option_value(key: &str, value: Option<OsString>) -> Result<OsString> {
    value.with_context(|| format!("{key} requires a value"))
}

fn required_value<I>(arguments: &mut I, name: &str) -> Result<OsString>
where
    I: Iterator<Item = OsString>,
{
    arguments
        .next()
        .with_context(|| format!("{name} requires a value"))
}

fn set_once_path(target: &mut Option<PathBuf>, value: OsString, name: &str) -> Result<()> {
    if target.replace(PathBuf::from(value)).is_some() {
        bail!("{name} may be specified only once");
    }
    Ok(())
}

fn set_once_string(target: &mut Option<String>, value: OsString, name: &str) -> Result<()> {
    let value = value
        .into_string()
        .map_err(|_| anyhow::anyhow!("{name} value must be valid Unicode"))?;
    if target.replace(value).is_some() {
        bail!("{name} may be specified only once");
    }
    Ok(())
}

fn require_no_arguments(arguments: &[OsString]) -> Result<()> {
    if arguments.is_empty() {
        Ok(())
    } else {
        bail!("this command does not accept arguments")
    }
}

pub const HELP: &str = r#"Talos Community appliance launcher

Usage:
  talos-server [--state-dir <absolute-path>] [--docker <absolute-path>] install --config <request.json> [--external-database-backup <file>]
  talos-server [--state-dir <absolute-path>] start|stop|status
  talos-server [--state-dir <absolute-path>] update --config <request.json> [--external-database-backup <file>]
  talos-server [--state-dir <absolute-path>] backup [--name <name>] [--external-database-backup <file>]
  talos-server [--state-dir <absolute-path>] restore <backup-name> --confirm <installation-id> [--external-database-restored]
  talos-server [--state-dir <absolute-path>] diagnostics [--name <name>]
  talos-server [--state-dir <absolute-path>] uninstall [--remove-data --confirm <installation-id>]

The protected install/update request contains non-secret settings and, for external PostgreSQL,
the database URL. Give that request owner-only permissions. Talos application images must be
digest-qualified. The launcher resolves traefik:latest only during install or explicit update.
"#;

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(arguments: &[&str]) -> Result<Cli> {
        Cli::parse(arguments.iter().map(OsString::from))
    }

    #[test]
    fn parses_documented_commands() {
        assert!(matches!(
            parse(&[
                "--state-dir",
                "/tmp/talos",
                "install",
                "--config",
                "/tmp/request.json"
            ])
            .expect("install")
            .command,
            CliCommand::Install { .. }
        ));
        assert!(matches!(
            parse(&[
                "--state-dir",
                "/tmp/talos",
                "restore",
                "daily-1",
                "--confirm",
                "abc"
            ])
            .expect("restore")
            .command,
            CliCommand::Restore { .. }
        ));
    }

    #[test]
    fn rejects_path_traversal_and_missing_destructive_confirmation() {
        assert!(parse(&[
            "--state-dir",
            "/tmp/talos",
            "restore",
            "../escape",
            "--confirm",
            "abc"
        ])
        .is_err());
        assert!(parse(&["--state-dir", "/tmp/talos", "uninstall", "--remove-data"]).is_err());
    }

    #[test]
    fn hostile_values_remain_values_not_new_options() {
        let cli = parse(&[
            "--state-dir",
            "/tmp/talos",
            "backup",
            "--external-database-backup",
            "/tmp/$(touch pwned)",
        ])
        .expect("path is opaque");
        match cli.command {
            CliCommand::Backup {
                external_database_backup,
                ..
            } => assert_eq!(
                external_database_backup,
                Some(PathBuf::from("/tmp/$(touch pwned)"))
            ),
            _ => panic!("wrong command"),
        }
    }
}
