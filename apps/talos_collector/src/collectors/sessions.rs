use crate::collectors::Collector;
use crate::models::{SessionsInfo, UserSessionInfo};
use crate::windows_utils::wmi::WmiHelper;
use anyhow::Result;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::collections::HashMap;
use tracing::debug;

pub struct SessionsCollector;

#[async_trait]
impl Collector for SessionsCollector {
    fn name(&self) -> &'static str {
        "Sessions"
    }

    fn data_type(&self) -> &'static str {
        "sessions"
    }

    fn estimated_duration_ms(&self) -> u64 {
        3000
    }

    async fn collect(&self) -> Result<Value> {
        debug!("Starting Sessions collection");

        let mut output = SessionsInfo::default();
        let sessions = WmiHelper::get_logon_sessions().await.unwrap_or_default();
        let links = WmiHelper::get_logged_on_user_links()
            .await
            .unwrap_or_default();

        let mut users_by_logon_id: HashMap<String, (String, Option<String>)> = HashMap::new();
        for link in links {
            let account = link
                .get("Antecedent")
                .and_then(|v| v.as_str())
                .and_then(parse_account_ref);
            let logon_id = link
                .get("Dependent")
                .and_then(|v| v.as_str())
                .and_then(parse_logon_id_ref);

            if let (Some((domain, user)), Some(id)) = (account, logon_id) {
                users_by_logon_id.insert(id, (user, Some(domain)));
            }
        }

        for s in sessions {
            let logon_id = value_to_string(s.get("LogonId"));
            if logon_id.is_empty() {
                continue;
            }

            let (user, domain) = users_by_logon_id
                .get(&logon_id)
                .cloned()
                .unwrap_or_else(|| ("Unknown".to_string(), None));

            let logon_type = match s.get("LogonType").and_then(|v| v.as_u64()).unwrap_or(0) {
                2 => "Interactive",
                3 => "Network",
                4 => "Batch",
                5 => "Service",
                7 => "Unlock",
                8 => "NetworkCleartext",
                10 => "RemoteInteractive",
                11 => "CachedInteractive",
                _ => "Other",
            }
            .to_string();

            output.sessions.push(UserSessionInfo {
                session_id: logon_id,
                user,
                domain,
                logon_type,
                logon_time: s
                    .get("StartTime")
                    .and_then(|v| v.as_str())
                    .and_then(WmiHelper::parse_wmi_datetime_str),
                authentication_package: s
                    .get("AuthenticationPackage")
                    .and_then(|v| v.as_str())
                    .map(|x| x.to_string()),
            });
        }

        debug!("Sessions collection completed");
        Ok(json!(output))
    }
}

fn parse_account_ref(input: &str) -> Option<(String, String)> {
    let domain = extract_ref_value(input, "Domain=\"")?;
    let name = extract_ref_value(input, "Name=\"")?;
    Some((domain, name))
}

fn parse_logon_id_ref(input: &str) -> Option<String> {
    extract_ref_value(input, "LogonId=\"")
}

fn extract_ref_value(input: &str, key: &str) -> Option<String> {
    let start = input.find(key)? + key.len();
    let rest = &input[start..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

fn value_to_string(v: Option<&Value>) -> String {
    if let Some(u) = v.and_then(|x| x.as_u64()) {
        return u.to_string();
    }
    if let Some(s) = v.and_then(|x| x.as_str()) {
        return s.to_string();
    }
    String::new()
}
