use std::{error::Error, net::SocketAddr, str::FromStr};

use anyhow::{anyhow, Context, Result};

const DEFAULT_BIND_PORT: u16 = 17110;
const DEFAULT_PING_INTERVAL_SECS: u64 = 25;
const DEFAULT_MAX_OUTPUT_BYTES: usize = 51_200;
const DEFAULT_MAX_EXECUTION_SECS: u64 = 60;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Config {
    pub(crate) bind_addr: SocketAddr,
    pub(crate) api_base_url: String,
    pub(crate) telemetry_producer_url: Option<String>,
    pub(crate) talos_server_api_key: Option<String>,
    pub(crate) cors_origins: Vec<String>,
    pub(crate) ping_interval_secs: u64,
    pub(crate) max_output_bytes: usize,
    pub(crate) max_execution_secs: u64,
    pub(crate) relay_url: Option<String>,
}

impl Config {
    pub(crate) fn patch_progress_url(&self) -> String {
        match &self.telemetry_producer_url {
            Some(base) => format!("{}/telemetry/patch/progress", base.trim_end_matches('/')),
            None => format!(
                "{}/rmm/telemetry/patch/progress",
                self.api_base_url.trim_end_matches('/')
            ),
        }
    }
}

pub(crate) fn load_config() -> Result<Config> {
    config_from_lookup(|key| std::env::var(key).ok())
}

fn config_from_lookup(mut lookup: impl FnMut(&str) -> Option<String>) -> Result<Config> {
    let bind_addr = match lookup("RMM_BIND_ADDR") {
        Some(value) => parse_explicit("RMM_BIND_ADDR", &value)?,
        None => SocketAddr::from(([127, 0, 0, 1], DEFAULT_BIND_PORT)),
    };

    let api_base_url = optional_trimmed(lookup("API_BACKEND_URL"))
        .ok_or_else(|| anyhow!("API_BACKEND_URL must be set"))?;

    let telemetry_producer_url = optional_trimmed(lookup("RMM_TELEMETRY_PRODUCER_URL"));
    let talos_server_api_key = lookup("RMM_SERVER_API_KEY");

    let cors_origins = lookup("RMM_CORS_ORIGINS")
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .collect();

    let ping_interval_secs = parse_or_default(
        lookup("RMM_PING_INTERVAL_SECS"),
        "RMM_PING_INTERVAL_SECS",
        DEFAULT_PING_INTERVAL_SECS,
    )?;
    let max_output_bytes = parse_or_default(
        lookup("RMM_MAX_OUTPUT_BYTES"),
        "RMM_MAX_OUTPUT_BYTES",
        DEFAULT_MAX_OUTPUT_BYTES,
    )?;
    let max_execution_secs = parse_or_default(
        lookup("RMM_MAX_EXECUTION_SECS"),
        "RMM_MAX_EXECUTION_SECS",
        DEFAULT_MAX_EXECUTION_SECS,
    )?;

    let relay_url = optional_trimmed(lookup("RMM_RELAY_URL"));

    Ok(Config {
        bind_addr,
        api_base_url,
        telemetry_producer_url,
        talos_server_api_key,
        cors_origins,
        ping_interval_secs,
        max_output_bytes,
        max_execution_secs,
        relay_url,
    })
}

fn optional_trimmed(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn parse_or_default<T>(value: Option<String>, key: &str, default: T) -> Result<T>
where
    T: FromStr,
    T::Err: Error + Send + Sync + 'static,
{
    match value {
        Some(value) => parse_explicit(key, &value),
        None => Ok(default),
    }
}

fn parse_explicit<T>(key: &str, value: &str) -> Result<T>
where
    T: FromStr,
    T::Err: Error + Send + Sync + 'static,
{
    value.trim().parse().with_context(|| format!("parse {key}"))
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn parse(values: &[(&str, &str)]) -> Result<Config> {
        let values = values.iter().copied().collect::<HashMap<_, _>>();
        config_from_lookup(|key| values.get(key).map(|value| (*value).to_string()))
    }

    #[test]
    fn missing_optional_values_use_defaults() {
        let config = parse(&[("API_BACKEND_URL", "http://api:3001")]).unwrap();

        assert_eq!(
            config.bind_addr,
            SocketAddr::from(([127, 0, 0, 1], DEFAULT_BIND_PORT))
        );
        assert_eq!(config.api_base_url, "http://api:3001");
        assert_eq!(config.telemetry_producer_url, None);
        assert_eq!(config.talos_server_api_key, None);
        assert!(config.cors_origins.is_empty());
        assert_eq!(config.ping_interval_secs, DEFAULT_PING_INTERVAL_SECS);
        assert_eq!(config.max_output_bytes, DEFAULT_MAX_OUTPUT_BYTES);
        assert_eq!(config.max_execution_secs, DEFAULT_MAX_EXECUTION_SECS);
        assert_eq!(config.relay_url, None);
    }

    #[test]
    fn trims_explicit_values_before_parsing() {
        let config = parse(&[
            ("API_BACKEND_URL", "  http://api:3001/  "),
            ("RMM_BIND_ADDR", "  127.0.0.1:3002  "),
            ("RMM_TELEMETRY_PRODUCER_URL", "  http://telemetry:17120  "),
            ("RMM_PING_INTERVAL_SECS", "  30  "),
            ("RMM_MAX_OUTPUT_BYTES", "  2048  "),
            ("RMM_MAX_EXECUTION_SECS", "  90  "),
            ("RMM_RELAY_URL", "  relay.example.test  "),
        ])
        .unwrap();

        assert_eq!(
            config.bind_addr,
            "127.0.0.1:3002".parse::<SocketAddr>().unwrap()
        );
        assert_eq!(config.api_base_url, "http://api:3001/");
        assert_eq!(
            config.telemetry_producer_url.as_deref(),
            Some("http://telemetry:17120")
        );
        assert_eq!(config.ping_interval_secs, 30);
        assert_eq!(config.max_output_bytes, 2048);
        assert_eq!(config.max_execution_secs, 90);
        assert_eq!(config.relay_url.as_deref(), Some("relay.example.test"));
    }

    #[test]
    fn api_backend_url_is_required_and_must_not_be_blank() {
        let missing = parse(&[]).unwrap_err();
        assert!(missing.to_string().contains("API_BACKEND_URL must be set"));

        let blank = parse(&[("API_BACKEND_URL", "   ")]).unwrap_err();
        assert!(blank.to_string().contains("API_BACKEND_URL must be set"));
    }

    #[test]
    fn malformed_bind_address_names_the_setting() {
        let error = parse(&[
            ("API_BACKEND_URL", "http://api:3001"),
            ("RMM_BIND_ADDR", "not-an-address"),
        ])
        .unwrap_err();

        assert!(error.to_string().contains("RMM_BIND_ADDR"));
    }

    #[test]
    fn malformed_explicit_numeric_values_name_the_setting() {
        for key in [
            "RMM_PING_INTERVAL_SECS",
            "RMM_MAX_OUTPUT_BYTES",
            "RMM_MAX_EXECUTION_SECS",
        ] {
            let error =
                parse(&[("API_BACKEND_URL", "http://api:3001"), (key, "invalid")]).unwrap_err();

            assert!(error.to_string().contains(key));
        }
    }

    #[test]
    fn comma_separated_cors_origins_are_trimmed_and_empty_entries_removed() {
        let config = parse(&[
            ("API_BACKEND_URL", "http://api:3001"),
            (
                "RMM_CORS_ORIGINS",
                " https://one.example, ,https://two.example , * ",
            ),
        ])
        .unwrap();

        assert_eq!(
            config.cors_origins,
            ["https://one.example", "https://two.example", "*"]
        );
    }

    #[test]
    fn patch_progress_has_one_authoritative_destination_per_topology() {
        let direct = parse(&[("API_BACKEND_URL", "http://api:3001/")]).unwrap();
        assert_eq!(
            direct.patch_progress_url(),
            "http://api:3001/rmm/telemetry/patch/progress"
        );

        let queued = parse(&[
            ("API_BACKEND_URL", "http://api:3001"),
            ("RMM_TELEMETRY_PRODUCER_URL", "http://producer:17120/"),
        ])
        .unwrap();
        assert_eq!(
            queued.patch_progress_url(),
            "http://producer:17120/telemetry/patch/progress"
        );
    }
}
