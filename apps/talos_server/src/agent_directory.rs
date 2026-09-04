use std::{collections::HashMap, sync::Arc};

use axum::extract::ws::Message;
use talos_protocol::{AgentFeatureCapabilities, AgentPlatform, LocalAddr};
use tokio::sync::{mpsc, RwLock, RwLockReadGuard};

#[derive(Clone)]
pub(crate) struct AgentConnection {
    pub(crate) sender: mpsc::Sender<Message>,
    pub(crate) host: Option<String>,
    pub(crate) local_addrs: Vec<LocalAddr>,
    pub(crate) is_admin: bool,
    pub(crate) hostname: Option<String>,
    pub(crate) os: Option<String>,
    pub(crate) version: Option<String>,
    pub(crate) platform: AgentPlatform,
    pub(crate) features: AgentFeatureCapabilities,
}

pub(crate) struct AgentRegistration {
    pub(crate) sender: mpsc::Sender<Message>,
    pub(crate) host: Option<String>,
    pub(crate) local_addrs: Option<Vec<LocalAddr>>,
    pub(crate) is_admin: Option<bool>,
    pub(crate) hostname: Option<String>,
    pub(crate) os: Option<String>,
    pub(crate) version: Option<String>,
    pub(crate) platform: AgentPlatform,
    pub(crate) features: AgentFeatureCapabilities,
}

#[derive(Clone, Default)]
pub(crate) struct AgentDirectory {
    entries: Arc<RwLock<HashMap<String, AgentConnection>>>,
}

impl AgentDirectory {
    pub(crate) async fn read(&self) -> RwLockReadGuard<'_, HashMap<String, AgentConnection>> {
        self.entries.read().await
    }

    pub(crate) async fn register(&self, agent_id: &str, registration: AgentRegistration) {
        let mut entries = self.entries.write().await;
        let entry = entries
            .entry(agent_id.to_string())
            .or_insert_with(|| AgentConnection {
                sender: registration.sender.clone(),
                host: None,
                local_addrs: Vec::new(),
                is_admin: false,
                hostname: None,
                os: None,
                version: None,
                platform: AgentPlatform::Unknown,
                features: AgentFeatureCapabilities::default(),
            });

        entry.sender = registration.sender;
        if registration.host.is_some() {
            entry.host = registration.host;
        }
        if let Some(local_addrs) = registration.local_addrs {
            if !local_addrs.is_empty() {
                entry.local_addrs = local_addrs;
            }
        }
        if let Some(is_admin) = registration.is_admin {
            entry.is_admin = is_admin;
        }
        if let Some(hostname) = registration.hostname {
            entry.hostname = Some(hostname);
        }
        if let Some(os) = registration.os {
            entry.os = Some(os);
        }
        if let Some(version) = registration.version {
            entry.version = Some(version);
        }
        entry.platform = registration.platform;
        entry.features = registration.features;
    }

    /// Removes an agent unless another socket has replaced `sender` in the directory.
    ///
    /// A replacement socket may register before the prior socket observes its close. In that
    /// case the prior socket no longer owns the directory entry and this returns `false`. A missing
    /// entry returns `true` so callers retain the existing cleanup behavior when no replacement is
    /// registered.
    pub(crate) async fn disconnect_unless_replaced(
        &self,
        agent_id: &str,
        sender: &mpsc::Sender<Message>,
    ) -> bool {
        let mut entries = self.entries.write().await;
        let Some(connection) = entries.get(agent_id) else {
            return true;
        };
        let owns_entry = connection.sender.same_channel(sender);

        if owns_entry {
            entries.remove(agent_id);
        }

        owns_entry
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registration(sender: mpsc::Sender<Message>, hostname: &str) -> AgentRegistration {
        AgentRegistration {
            sender,
            host: None,
            local_addrs: None,
            is_admin: None,
            hostname: Some(hostname.to_string()),
            os: None,
            version: None,
            platform: AgentPlatform::Unknown,
            features: AgentFeatureCapabilities::default(),
        }
    }

    #[tokio::test]
    async fn stale_socket_disconnect_does_not_remove_replacement() {
        let directory = AgentDirectory::default();
        let (old_sender, _old_receiver) = mpsc::channel(1);
        let (replacement_sender, _replacement_receiver) = mpsc::channel(1);

        directory
            .register(
                "agent-1",
                registration(old_sender.clone(), "old-connection"),
            )
            .await;
        directory
            .register(
                "agent-1",
                registration(replacement_sender.clone(), "replacement-connection"),
            )
            .await;

        assert!(
            !directory
                .disconnect_unless_replaced("agent-1", &old_sender)
                .await
        );

        let entries = directory.read().await;
        let current = entries
            .get("agent-1")
            .expect("replacement remains registered");
        assert!(current.sender.same_channel(&replacement_sender));
        assert_eq!(current.hostname.as_deref(), Some("replacement-connection"));
        drop(entries);

        assert!(
            directory
                .disconnect_unless_replaced("agent-1", &replacement_sender)
                .await
        );
        assert!(directory.read().await.get("agent-1").is_none());
    }

    #[tokio::test]
    async fn absent_entry_does_not_block_existing_disconnect_cleanup() {
        let directory = AgentDirectory::default();
        let (sender, _receiver) = mpsc::channel(1);

        assert!(
            directory
                .disconnect_unless_replaced("agent-1", &sender)
                .await
        );
    }
}
