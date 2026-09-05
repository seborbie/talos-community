use std::{
    collections::HashMap,
    net::Ipv4Addr,
    path::{Component, Path, PathBuf},
    str::FromStr,
};

use anyhow::{bail, Context, Result};
use rand::{rngs::OsRng, RngCore};
use serde::{Deserialize, Serialize};
use url::Url;

pub const CONFIG_SCHEMA_VERSION: u32 = 1;
pub const DEFAULT_EDGE_SUBNET: &str = "172.31.240.0/24";
pub const DEFAULT_TRAEFIK_IPV4: &str = "172.31.240.2";

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstallRequest {
    pub schema_version: u32,
    pub release_version: String,
    #[serde(default = "default_update_channel")]
    pub update_channel: String,
    pub images: ReleaseImages,
    pub database: DatabaseRequest,
    pub edge: EdgeConfig,
    #[serde(default)]
    pub paths: PathRequest,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum DatabaseRequest {
    Bundled {
        #[serde(default = "default_postgres_user")]
        user: String,
        #[serde(default = "default_postgres_database")]
        database: String,
    },
    External {
        url: String,
    },
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PathRequest {
    pub backup_directory: Option<PathBuf>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct InstallationConfig {
    pub schema_version: u32,
    pub release_version: String,
    pub update_channel: String,
    pub installation_root: PathBuf,
    pub backup_directory: PathBuf,
    pub images: ReleaseImages,
    pub database: DatabaseConfig,
    pub edge: EdgeConfig,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum DatabaseConfig {
    Bundled { user: String, database: String },
    External { identity: ExternalDatabaseIdentity },
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ExternalDatabaseIdentity {
    pub scheme: String,
    pub host: String,
    pub port: u16,
    pub database: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ReleaseImages {
    pub api_backend: String,
    pub frontend: String,
    pub relay: String,
    pub control_server: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct EdgeConfig {
    pub mode: EdgeMode,
    pub frontend_domain: String,
    pub api_domain: String,
    pub control_domain: String,
    pub relay_domain: String,
    #[serde(default)]
    pub acme_email: Option<String>,
    #[serde(default)]
    pub acme_ca_server: Option<String>,
    #[serde(default)]
    pub certificate_path: Option<PathBuf>,
    #[serde(default)]
    pub private_key_path: Option<PathBuf>,
    #[serde(default = "default_http_port")]
    pub http_port: u16,
    #[serde(default = "default_https_port")]
    pub https_port: u16,
    #[serde(default = "default_edge_subnet")]
    pub subnet: String,
    #[serde(default = "default_traefik_ipv4")]
    pub proxy_ipv4: String,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EdgeMode {
    PublicAcme,
    CustomCertificate,
    Local,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SecretConfig {
    pub schema_version: u32,
    pub jwt_secret: String,
    pub app_encryption_key: String,
    pub rmm_server_api_key: String,
    #[serde(default)]
    pub postgres_password: Option<String>,
    #[serde(default)]
    pub external_database_url: Option<String>,
}

fn default_update_channel() -> String {
    "stable".to_string()
}

fn default_postgres_user() -> String {
    "talos".to_string()
}

fn default_postgres_database() -> String {
    "talos".to_string()
}

fn default_http_port() -> u16 {
    80
}

fn default_https_port() -> u16 {
    443
}

fn default_edge_subnet() -> String {
    DEFAULT_EDGE_SUBNET.to_string()
}

fn default_traefik_ipv4() -> String {
    DEFAULT_TRAEFIK_IPV4.to_string()
}

impl InstallRequest {
    pub fn load(path: &Path, state_root: &Path) -> Result<(InstallationConfig, SecretInput)> {
        let bytes = crate::secure_fs::read_protected_file(path, 1024 * 1024)
            .with_context(|| format!("could not read installation request {}", path.display()))?;
        let request: Self = serde_json::from_slice(&bytes)
            .context("installation request is not valid schema-versioned JSON")?;
        request.validate_and_split(state_root)
    }

    pub fn validate_and_split(
        self,
        state_root: &Path,
    ) -> Result<(InstallationConfig, SecretInput)> {
        if self.schema_version != CONFIG_SCHEMA_VERSION {
            bail!(
                "unsupported configuration schema {}; expected {}",
                self.schema_version,
                CONFIG_SCHEMA_VERSION
            );
        }
        validate_safe_token("release_version", &self.release_version, 64)?;
        if self.update_channel != "stable" {
            bail!("update_channel must be 'stable' in this release");
        }
        self.images.validate()?;
        self.edge.validate()?;
        validate_absolute_path("installation root", state_root)?;

        let backup_directory = self
            .paths
            .backup_directory
            .unwrap_or_else(|| state_root.join("backups"));
        validate_absolute_path("backup_directory", &backup_directory)?;

        let (database, secret_input) = match self.database {
            DatabaseRequest::Bundled { user, database } => {
                validate_postgres_identifier("database user", &user)?;
                validate_postgres_identifier("database name", &database)?;
                (
                    DatabaseConfig::Bundled { user, database },
                    SecretInput {
                        external_database_url: None,
                    },
                )
            }
            DatabaseRequest::External { url } => {
                let identity = ExternalDatabaseIdentity::from_url(&url)?;
                (
                    DatabaseConfig::External { identity },
                    SecretInput {
                        external_database_url: Some(url),
                    },
                )
            }
        };

        Ok((
            InstallationConfig {
                schema_version: CONFIG_SCHEMA_VERSION,
                release_version: self.release_version,
                update_channel: self.update_channel,
                installation_root: state_root.to_path_buf(),
                backup_directory,
                images: self.images,
                database,
                edge: self.edge,
            },
            secret_input,
        ))
    }
}

#[derive(Clone, Debug)]
pub struct SecretInput {
    pub external_database_url: Option<String>,
}

impl SecretConfig {
    pub fn generate(config: &InstallationConfig, input: SecretInput) -> Self {
        Self {
            schema_version: CONFIG_SCHEMA_VERSION,
            jwt_secret: random_hex(32),
            app_encryption_key: random_hex(32),
            rmm_server_api_key: random_hex(32),
            postgres_password: matches!(&config.database, DatabaseConfig::Bundled { .. })
                .then(|| random_hex(32)),
            external_database_url: input.external_database_url,
        }
    }

    pub fn update_database_secret(&mut self, config: &InstallationConfig, input: SecretInput) {
        match &config.database {
            DatabaseConfig::Bundled { .. } => {
                if self.postgres_password.is_none() {
                    self.postgres_password = Some(random_hex(32));
                }
                self.external_database_url = None;
            }
            DatabaseConfig::External { .. } => {
                self.postgres_password = None;
                if input.external_database_url.is_some() {
                    self.external_database_url = input.external_database_url;
                }
            }
        }
    }

    pub fn validate_for(&self, config: &InstallationConfig) -> Result<()> {
        if self.schema_version != CONFIG_SCHEMA_VERSION {
            bail!("unsupported protected-secret schema");
        }
        for (label, value) in [
            ("JWT secret", &self.jwt_secret),
            ("application encryption key", &self.app_encryption_key),
            ("RMM server API key", &self.rmm_server_api_key),
        ] {
            if value.len() < 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                bail!("{label} is malformed");
            }
        }
        match &config.database {
            DatabaseConfig::Bundled { .. } => {
                let password = self
                    .postgres_password
                    .as_deref()
                    .context("bundled PostgreSQL password is missing")?;
                if password.len() < 32 || !password.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                    bail!("bundled PostgreSQL password is malformed");
                }
            }
            DatabaseConfig::External { identity } => {
                let url = self
                    .external_database_url
                    .as_deref()
                    .context("external PostgreSQL URL is missing")?;
                let actual_identity = ExternalDatabaseIdentity::from_url(url)?;
                if &actual_identity != identity {
                    bail!(
                        "external PostgreSQL URL identity does not match the protected installation configuration"
                    );
                }
            }
        }
        Ok(())
    }

    pub fn redaction_values(&self) -> Vec<&str> {
        let mut values = vec![
            self.jwt_secret.as_str(),
            self.app_encryption_key.as_str(),
            self.rmm_server_api_key.as_str(),
        ];
        if let Some(value) = self.postgres_password.as_deref() {
            values.push(value);
        }
        if let Some(value) = self.external_database_url.as_deref() {
            values.push(value);
        }
        values
    }
}

impl ReleaseImages {
    pub fn validate(&self) -> Result<()> {
        for (label, image) in [
            ("api_backend", &self.api_backend),
            ("frontend", &self.frontend),
            ("relay", &self.relay),
            ("control_server", &self.control_server),
        ] {
            validate_digest_image(label, image)?;
        }
        Ok(())
    }

    pub fn values(&self) -> [&str; 4] {
        [
            &self.api_backend,
            &self.frontend,
            &self.relay,
            &self.control_server,
        ]
    }
}

impl EdgeConfig {
    pub fn validate(&self) -> Result<()> {
        for (label, domain) in [
            ("frontend_domain", &self.frontend_domain),
            ("api_domain", &self.api_domain),
            ("control_domain", &self.control_domain),
            ("relay_domain", &self.relay_domain),
        ] {
            validate_domain(label, domain)?;
        }
        let mut unique = std::collections::HashSet::new();
        for domain in [
            &self.frontend_domain,
            &self.api_domain,
            &self.control_domain,
            &self.relay_domain,
        ] {
            if !unique.insert(domain.to_ascii_lowercase()) {
                bail!("each public route must use a distinct domain");
            }
        }
        if self.http_port == 0 || self.https_port == 0 || self.http_port == self.https_port {
            bail!("edge HTTP and HTTPS ports must be distinct non-zero ports");
        }
        validate_proxy_network(&self.subnet, &self.proxy_ipv4)?;

        match self.mode {
            EdgeMode::PublicAcme => {
                if self.http_port != 80 || self.https_port != 443 {
                    bail!(
                        "public_acme mode requires host ports 80 and 443; use external NAT without changing these launcher ports"
                    );
                }
                let email = self
                    .acme_email
                    .as_deref()
                    .context("public_acme mode requires acme_email")?;
                validate_email(email)?;
                if let Some(server) = self.acme_ca_server.as_deref() {
                    validate_https_url("acme_ca_server", server)?;
                }
                if self.certificate_path.is_some() || self.private_key_path.is_some() {
                    bail!("public_acme mode must not set certificate paths");
                }
            }
            EdgeMode::CustomCertificate => {
                if self.acme_email.is_some() || self.acme_ca_server.is_some() {
                    bail!("custom_certificate mode must not set ACME fields");
                }
                validate_absolute_path(
                    "certificate_path",
                    self.certificate_path
                        .as_deref()
                        .context("custom_certificate mode requires certificate_path")?,
                )?;
                validate_absolute_path(
                    "private_key_path",
                    self.private_key_path
                        .as_deref()
                        .context("custom_certificate mode requires private_key_path")?,
                )?;
            }
            EdgeMode::Local => {
                if self.acme_email.is_some()
                    || self.acme_ca_server.is_some()
                    || self.certificate_path.is_some()
                    || self.private_key_path.is_some()
                {
                    bail!("local mode generates its own certificate and must not set certificate or ACME fields");
                }
            }
        }
        Ok(())
    }
}

pub fn validate_digest_image(label: &str, image: &str) -> Result<()> {
    if image.len() > 512 || image.chars().any(char::is_whitespace) || image.contains(['\'', '"']) {
        bail!("{label} image reference is malformed");
    }
    let Some((repository, digest)) = image.rsplit_once("@sha256:") else {
        bail!("{label} image must be digest-qualified with @sha256");
    };
    if repository.is_empty()
        || repository.starts_with('-')
        || !repository.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'/' | b'_' | b'-' | b':')
        })
    {
        bail!("{label} image repository is malformed");
    }
    if digest.len() != 64 || !digest.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        bail!("{label} image digest must contain exactly 64 hexadecimal characters");
    }
    Ok(())
}

pub fn validate_external_database_url(value: &str) -> Result<()> {
    validated_external_database_url(value).map(|_| ())
}

impl ExternalDatabaseIdentity {
    pub fn from_url(value: &str) -> Result<Self> {
        let parsed = validated_external_database_url(value)?;
        Ok(Self {
            scheme: "postgresql".to_string(),
            host: parsed
                .host_str()
                .context("external PostgreSQL URL must identify a host")?
                .to_ascii_lowercase(),
            port: parsed.port().unwrap_or(5432),
            database: parsed.path().trim_start_matches('/').to_string(),
        })
    }
}

fn validated_external_database_url(value: &str) -> Result<Url> {
    if value.len() > 4096
        || value.chars().any(|character| character.is_control())
        || value.contains(['\'', '"', '$', '\\'])
    {
        bail!("external PostgreSQL URL contains unsupported characters");
    }
    let parsed = Url::parse(value).context("external PostgreSQL URL is malformed")?;
    if !matches!(parsed.scheme(), "postgres" | "postgresql") {
        bail!("external PostgreSQL URL must use postgres or postgresql");
    }
    if parsed.username().is_empty() || parsed.host_str().is_none() {
        bail!("external PostgreSQL URL must identify a user and host");
    }
    if parsed.path().trim_matches('/').is_empty() {
        bail!("external PostgreSQL URL must identify a database");
    }
    if parsed.fragment().is_some() {
        bail!("external PostgreSQL URL must not contain a fragment");
    }
    let query_pairs: Vec<_> = parsed.query_pairs().into_owned().collect();
    for (key, _) in &query_pairs {
        if matches!(
            key.to_ascii_lowercase().as_str(),
            "database" | "dbname" | "host" | "hostaddr" | "port" | "service" | "servicefile"
        ) {
            bail!(
                "external PostgreSQL URL must express database identity in its authority and path, not query parameters"
            );
        }
    }
    for required in ["sslmode", "connect_timeout"] {
        if query_pairs
            .iter()
            .filter(|(key, _)| key == required)
            .count()
            != 1
        {
            bail!("external PostgreSQL URL requires exactly one {required} parameter");
        }
    }
    let query: HashMap<_, _> = query_pairs.into_iter().collect();
    if !matches!(
        query.get("sslmode").map(String::as_str),
        Some("require" | "verify-ca" | "verify-full")
    ) {
        bail!("external PostgreSQL URL requires sslmode=require, verify-ca, or verify-full");
    }
    let timeout = query
        .get("connect_timeout")
        .context("external PostgreSQL URL requires connect_timeout")?
        .parse::<u8>()
        .context("external PostgreSQL connect_timeout must be an integer")?;
    if !(1..=30).contains(&timeout) {
        bail!("external PostgreSQL connect_timeout must be between 1 and 30 seconds");
    }
    Ok(parsed)
}

fn validate_proxy_network(subnet: &str, proxy: &str) -> Result<()> {
    let (network_text, prefix_text) = subnet
        .split_once('/')
        .context("edge subnet must use IPv4 CIDR notation")?;
    let network = Ipv4Addr::from_str(network_text).context("edge subnet address is invalid")?;
    let prefix = prefix_text
        .parse::<u8>()
        .context("edge subnet prefix is invalid")?;
    if !(16..=28).contains(&prefix) {
        bail!("edge subnet prefix must be between /16 and /28");
    }
    let proxy = Ipv4Addr::from_str(proxy).context("Traefik proxy IPv4 address is invalid")?;
    let mask = u32::MAX << (32 - prefix);
    let network_number = u32::from(network);
    let proxy_number = u32::from(proxy);
    if network_number & !mask != 0 {
        bail!("edge subnet must use its canonical network address");
    }
    if proxy_number & mask != network_number
        || proxy_number == network_number
        || proxy_number == network_number | !mask
    {
        bail!("Traefik proxy IPv4 address must be a usable address inside the edge subnet");
    }
    Ok(())
}

fn validate_domain(label: &str, value: &str) -> Result<()> {
    if value.is_empty()
        || value.len() > 253
        || value.ends_with('.')
        || !value.is_ascii()
        || value.parse::<Ipv4Addr>().is_ok()
    {
        bail!("{label} must be a DNS hostname without a trailing dot");
    }
    for part in value.split('.') {
        if part.is_empty()
            || part.len() > 63
            || part.starts_with('-')
            || part.ends_with('-')
            || !part
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        {
            bail!("{label} contains an invalid DNS label");
        }
    }
    Ok(())
}

fn validate_email(value: &str) -> Result<()> {
    if value.len() > 254
        || value
            .chars()
            .any(|character| character.is_control() || character.is_whitespace())
        || value.matches('@').count() != 1
    {
        bail!("acme_email is malformed");
    }
    let (_, domain) = value.split_once('@').context("acme_email is malformed")?;
    validate_domain("ACME email domain", domain)
}

fn validate_https_url(label: &str, value: &str) -> Result<()> {
    if value.len() > 2048 || value.chars().any(|character| character.is_control()) {
        bail!("{label} is malformed");
    }
    let parsed = Url::parse(value).with_context(|| format!("{label} is malformed"))?;
    if parsed.scheme() != "https" || parsed.host_str().is_none() || parsed.username() != "" {
        bail!("{label} must be an HTTPS URL without credentials");
    }
    Ok(())
}

fn validate_postgres_identifier(label: &str, value: &str) -> Result<()> {
    if value.is_empty()
        || value.len() > 63
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        || !value
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphabetic)
    {
        bail!("{label} must be a safe PostgreSQL identifier");
    }
    Ok(())
}

fn validate_safe_token(label: &str, value: &str, max_len: usize) -> Result<()> {
    if value.is_empty()
        || value.len() > max_len
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'+' | b'-'))
    {
        bail!("{label} contains unsupported characters");
    }
    Ok(())
}

pub fn validate_absolute_path(label: &str, path: &Path) -> Result<()> {
    if !path.is_absolute() {
        bail!("{label} must be an absolute path");
    }
    for component in path.components() {
        if matches!(component, Component::CurDir | Component::ParentDir) {
            bail!("{label} must not contain traversal components");
        }
    }
    if path
        .to_string_lossy()
        .chars()
        .any(|character| character.is_control() || character == '\'')
    {
        bail!("{label} contains characters unsupported by the Compose environment file");
    }
    Ok(())
}

fn random_hex(byte_count: usize) -> String {
    let mut bytes = vec![0_u8; byte_count];
    OsRng.fill_bytes(&mut bytes);
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(byte_count * 2);
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    fn digest(name: &str, byte: char) -> String {
        format!(
            "registry.example/talos/{name}@sha256:{}",
            byte.to_string().repeat(64)
        )
    }

    fn request() -> InstallRequest {
        InstallRequest {
            schema_version: 1,
            release_version: "1.2.3".to_string(),
            update_channel: "stable".to_string(),
            images: ReleaseImages {
                api_backend: digest("api", 'a'),
                frontend: digest("frontend", 'b'),
                relay: digest("relay", 'c'),
                control_server: digest("server", 'd'),
            },
            database: DatabaseRequest::Bundled {
                user: "talos".to_string(),
                database: "talos".to_string(),
            },
            edge: EdgeConfig {
                mode: EdgeMode::PublicAcme,
                frontend_domain: "talos.example.com".to_string(),
                api_domain: "api.talos.example.com".to_string(),
                control_domain: "control.talos.example.com".to_string(),
                relay_domain: "relay.talos.example.com".to_string(),
                acme_email: Some("admin@example.com".to_string()),
                acme_ca_server: None,
                certificate_path: None,
                private_key_path: None,
                http_port: 80,
                https_port: 443,
                subnet: DEFAULT_EDGE_SUBNET.to_string(),
                proxy_ipv4: DEFAULT_TRAEFIK_IPV4.to_string(),
            },
            paths: PathRequest::default(),
        }
    }

    #[test]
    fn validates_and_removes_external_database_url_from_persistent_config() {
        let database_url =
            "postgresql://talos:encoded%21@db.example.com/talos?sslmode=verify-full&connect_timeout=5";
        let mut input = request();
        input.database = DatabaseRequest::External {
            url: database_url.to_string(),
        };
        let (config, secret) = input
            .validate_and_split(&std::env::temp_dir().join("talos-server-config-test"))
            .expect("valid request");

        let serialized = serde_json::to_string(&config).expect("serialize");
        assert!(matches!(config.database, DatabaseConfig::External { .. }));
        assert!(!serialized.contains(database_url));
        assert!(!serialized.contains("encoded%21"));
        assert!(serialized.contains("db.example.com"));
        assert!(serialized.contains("talos"));
        assert_eq!(secret.external_database_url.as_deref(), Some(database_url));
    }

    #[test]
    fn external_database_identity_is_canonical_and_excludes_credentials() {
        let first = ExternalDatabaseIdentity::from_url(
            "postgres://first:password-one@DB.EXAMPLE.COM/talos?sslmode=require&connect_timeout=5",
        )
        .expect("first identity");
        let rotated = ExternalDatabaseIdentity::from_url(
            "postgresql://second:password-two@db.example.com:5432/talos?connect_timeout=10&sslmode=verify-full",
        )
        .expect("rotated identity");

        assert_eq!(first, rotated, "credential rotation must retain identity");
        let serialized = serde_json::to_string(&first).expect("serialize identity");
        assert!(!serialized.contains("first"));
        assert!(!serialized.contains("password"));
        assert_eq!(first.scheme, "postgresql");
        assert_eq!(first.host, "db.example.com");
        assert_eq!(first.port, 5432);
        assert_eq!(first.database, "talos");

        for different in [
            "postgresql://first:password@other.example.com/talos?sslmode=require&connect_timeout=5",
            "postgresql://first:password@db.example.com:5433/talos?sslmode=require&connect_timeout=5",
            "postgresql://first:password@db.example.com/other?sslmode=require&connect_timeout=5",
        ] {
            assert_ne!(
                first,
                ExternalDatabaseIdentity::from_url(different).expect("different identity")
            );
        }
    }

    #[test]
    fn external_database_secret_must_match_persisted_non_secret_identity() {
        let mut input = request();
        input.database = DatabaseRequest::External {
            url: "postgresql://talos:old-password@db.example.com/talos?sslmode=verify-full&connect_timeout=5".to_string(),
        };
        let (config, secret_input) = input
            .validate_and_split(&std::env::temp_dir().join("talos-server-config-test"))
            .expect("valid request");
        let mut secrets = SecretConfig::generate(&config, secret_input);
        secrets.external_database_url = Some(
            "postgresql://talos:new-password@other.example.com/talos?sslmode=verify-full&connect_timeout=5".to_string(),
        );

        let error = secrets
            .validate_for(&config)
            .expect_err("identity mismatch must fail")
            .to_string();
        assert!(error.contains("identity does not match"));
        assert!(!error.contains("old-password"));
        assert!(!error.contains("new-password"));
        assert!(!error.contains("other.example.com"));
    }

    #[test]
    fn rejects_tag_only_and_hostile_image_references() {
        assert!(validate_digest_image("api", "example/api:latest").is_err());
        assert!(validate_digest_image(
            "api",
            &format!("example/api;touch-pwned@sha256:{}", "a".repeat(64))
        )
        .is_err());
    }

    #[test]
    fn rejects_insecure_or_dotenv_hostile_database_urls_without_echoing_them() {
        let insecure =
            "postgresql://user:password@db.example/talos?sslmode=disable&connect_timeout=5";
        let error = validate_external_database_url(insecure)
            .expect_err("insecure URL must fail")
            .to_string();
        assert!(!error.contains("password"));

        assert!(validate_external_database_url(
            "postgresql://user:pa$ss@db.example/talos?sslmode=require&connect_timeout=5"
        )
        .is_err());
        for override_parameter in ["host=other.example", "port=5433", "dbname=other"] {
            assert!(validate_external_database_url(&format!(
                "postgresql://user:password@db.example/talos?sslmode=require&connect_timeout=5&{override_parameter}"
            ))
            .is_err());
        }
    }

    #[test]
    fn rejects_duplicate_routes_and_proxy_outside_subnet() {
        let mut input = request();
        input.edge.relay_domain = input.edge.control_domain.clone();
        assert!(input
            .validate_and_split(&std::env::temp_dir().join("talos-server-config-test"))
            .is_err());

        let mut input = request();
        input.edge.proxy_ipv4 = "172.31.241.2".to_string();
        assert!(input
            .validate_and_split(&std::env::temp_dir().join("talos-server-config-test"))
            .is_err());
    }

    #[test]
    fn secrets_are_independent_csprng_values() {
        let (config, input) = request()
            .validate_and_split(&std::env::temp_dir().join("talos-server-config-test"))
            .expect("valid request");
        let secrets = SecretConfig::generate(&config, input);
        secrets.validate_for(&config).expect("valid secrets");

        assert_ne!(secrets.jwt_secret, secrets.app_encryption_key);
        assert_ne!(secrets.jwt_secret, secrets.rmm_server_api_key);
        assert_ne!(
            secrets.postgres_password.as_deref(),
            Some(secrets.jwt_secret.as_str())
        );
    }

    #[test]
    fn tracked_example_matches_the_versioned_schema() {
        let parsed: InstallRequest =
            serde_json::from_str(include_str!("../talos-server.example.json"))
                .expect("example JSON");
        parsed
            .validate_and_split(&std::env::temp_dir().join("talos-server-config-test"))
            .expect("example contract");
    }

    #[test]
    fn public_acme_rejects_nonstandard_host_ports_without_an_explicit_nat_model() {
        let mut input = request();
        input.edge.http_port = 8080;
        assert!(input
            .validate_and_split(&std::env::temp_dir().join("talos-server-config-test"))
            .is_err());
    }
}
