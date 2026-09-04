use crate::collectors::Collector;
use crate::models::{CertificateInfo, CertificateStoreInfo, CertificatesInfo};
use anyhow::Result;
use async_trait::async_trait;
#[cfg(windows)]
use chrono::DateTime;
use chrono::Utc;
use serde_json::{json, Value};
use tracing::debug;

pub struct CertificatesCollector;

#[async_trait]
impl Collector for CertificatesCollector {
    fn name(&self) -> &'static str {
        "Certificates"
    }

    fn data_type(&self) -> &'static str {
        "certificates"
    }

    fn estimated_duration_ms(&self) -> u64 {
        4000
    }

    async fn collect(&self) -> Result<Value> {
        debug!("Starting Certificates collection");

        let mut certs = CertificatesInfo::default();

        for store_name in ["MY", "CA", "ROOT"] {
            certs
                .stores
                .push(self.collect_store("LocalMachine", store_name));
            certs
                .stores
                .push(self.collect_store("CurrentUser", store_name));
        }

        let now = Utc::now();
        let cutoff = now + chrono::Duration::days(30);
        certs.certificates_expiring_30d = certs
            .stores
            .iter()
            .flat_map(|s| s.certificates.iter())
            .filter(|c| {
                c.not_after
                    .map(|dt| dt >= now && dt <= cutoff)
                    .unwrap_or(false)
            })
            .count() as u32;

        debug!("Certificates collection completed");
        Ok(json!(certs))
    }
}

impl CertificatesCollector {
    fn collect_store(&self, location: &str, store_name: &str) -> CertificateStoreInfo {
        CertificateStoreInfo {
            location: location.to_string(),
            store_name: store_name.to_string(),
            certificates: self.read_store_certs(location, store_name),
        }
    }

    #[cfg(windows)]
    fn read_store_certs(&self, location: &str, store_name: &str) -> Vec<CertificateInfo> {
        use sha1::{Digest, Sha1};
        use x509_parser::prelude::{FromDer, X509Certificate};

        let store = match location {
            "LocalMachine" => schannel::cert_store::CertStore::open_local_machine(store_name),
            _ => schannel::cert_store::CertStore::open_current_user(store_name),
        };

        let Ok(store) = store else {
            return Vec::new();
        };

        store
            .certs()
            .filter_map(|ctx| {
                let der = ctx.to_der();
                let thumbprint = {
                    let mut hasher = Sha1::new();
                    hasher.update(der);
                    format!("{:x}", hasher.finalize())
                };

                let parsed = X509Certificate::from_der(der).ok()?;
                let cert = parsed.1;

                Some(CertificateInfo {
                    thumbprint,
                    subject: cert.subject().to_string(),
                    issuer: cert.issuer().to_string(),
                    not_before: self.parse_asn1_timestamp(cert.validity().not_before.timestamp()),
                    not_after: self.parse_asn1_timestamp(cert.validity().not_after.timestamp()),
                })
            })
            .collect()
    }

    #[cfg(not(windows))]
    fn read_store_certs(&self, _location: &str, _store_name: &str) -> Vec<CertificateInfo> {
        Vec::new()
    }

    #[cfg(windows)]
    fn parse_asn1_timestamp(&self, ts: i64) -> Option<DateTime<Utc>> {
        DateTime::from_timestamp(ts, 0)
    }
}
