use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha1::{Digest, Sha1};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventEnvelope {
    pub seq: u64,
    pub ts: DateTime<Utc>,
    pub event_type: String,
    pub event_kind: String,
    pub scope_key: String,
    pub data: Value,
    pub hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventInput {
    pub event_type: String,
    pub event_kind: String,
    pub scope_key: String,
    pub data: Value,
}

impl EventInput {
    pub fn new(
        event_type: impl Into<String>,
        event_kind: impl Into<String>,
        scope_key: impl Into<String>,
        data: Value,
    ) -> Self {
        Self {
            event_type: event_type.into(),
            event_kind: event_kind.into(),
            scope_key: scope_key.into(),
            data,
        }
    }
}

pub fn compute_data_hash(data: &Value) -> String {
    let canonical = canonicalize_json(data);
    let encoded = serde_json::to_vec(&canonical).unwrap_or_default();
    let mut hasher = Sha1::new();
    hasher.update(encoded);
    format!("{:x}", hasher.finalize())
}

fn canonicalize_json(value: &Value) -> Value {
    match value {
        Value::Object(obj) => {
            let mut keys: Vec<&String> = obj.keys().collect();
            keys.sort_unstable();
            let mut map = Map::new();
            for key in keys {
                if let Some(v) = obj.get(key) {
                    map.insert(key.clone(), canonicalize_json(v));
                }
            }
            Value::Object(map)
        }
        Value::Array(items) => Value::Array(items.iter().map(canonicalize_json).collect()),
        _ => value.clone(),
    }
}
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha1::{Digest, Sha1};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventEnvelope {
    pub seq: u64,
    pub ts: DateTime<Utc>,
    pub event_type: String,
    pub event_kind: String,
    pub scope_key: String,
    pub data: Value,
    pub hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventInput {
    pub event_type: String,
    pub event_kind: String,
    pub scope_key: String,
    pub data: Value,
}

impl EventInput {
    pub fn new(
        event_type: impl Into<String>,
        event_kind: impl Into<String>,
        scope_key: impl Into<String>,
        data: Value,
    ) -> Self {
        Self {
            event_type: event_type.into(),
            event_kind: event_kind.into(),
            scope_key: scope_key.into(),
            data,
        }
    }
}

pub fn compute_data_hash(data: &Value) -> String {
    let canonical = canonicalize_json(data);
    let encoded = serde_json::to_vec(&canonical).unwrap_or_default();
    let mut hasher = Sha1::new();
    hasher.update(encoded);
    format!("{:x}", hasher.finalize())
}

fn canonicalize_json(value: &Value) -> Value {
    match value {
        Value::Object(obj) => {
            let mut keys: Vec<&String> = obj.keys().collect();
            keys.sort_unstable();
            let mut map = Map::new();
            for key in keys {
                if let Some(v) = obj.get(key) {
                    map.insert(key.clone(), canonicalize_json(v));
                }
            }
            Value::Object(map)
        }
        Value::Array(items) => Value::Array(items.iter().map(canonicalize_json).collect()),
        _ => value.clone(),
    }
}
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventEnvelope {
    pub seq: u64,
    pub ts: String,
    pub event_type: String,
    pub event_kind: String,
    pub scope_key: String,
    pub data: Value,
    pub hash: String,
}

impl EventEnvelope {
    pub fn new(event_type: &str, event_kind: &str, scope_key: impl Into<String>, data: Value) -> Self {
        Self {
            seq: 0,
            ts: String::new(),
            event_type: event_type.to_string(),
            event_kind: event_kind.to_string(),
            scope_key: scope_key.into(),
            data,
            hash: String::new(),
        }
    }
}
//! Event schema for the event stream
//!
//! Each event includes:
//! - seq: monotonic sequence number per process
//! - ts: ISO8601 timestamp
//! - event_type: category (service, software, network, etc.)
//! - event_kind: specific occurrence (state_changed, installed, etc.)
//! - scope_key: unique identifier for the changed object
//! - data: event-specific payload
//! - hash: SHA-256 of canonical data JSON for dedupe

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::sync::atomic::{AtomicU64, Ordering};

/// Global sequence counter (starts at 1 per process)
static SEQ_COUNTER: AtomicU64 = AtomicU64::new(1);

/// Event envelope - all events use this structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    /// Monotonic sequence number (per agent run)
    pub seq: u64,
    /// ISO8601 timestamp
    pub ts: String,
    /// Event category/type (e.g., "service", "software", "network")
    pub event_type: String,
    /// Event kind/specific occurrence (e.g., "state_changed", "installed")
    pub event_kind: String,
    /// Scope key - unique identity of changed object (e.g., "service:Spooler")
    pub scope_key: String,
    /// Event-specific payload
    pub data: Value,
    /// Hash of data payload for deduplication
    pub hash: String,
    /// Optional: agent metadata for context
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    /// Optional: device name for context
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device_name: Option<String>,
    /// Optional: boot session ID for grouping events per boot
    #[serde(skip_serializing_if = "Option::is_none")]
    pub boot_session_id: Option<String>,
}

impl Event {
    /// Create a new event with auto-incremented sequence number
    pub fn new(
        event_type: impl Into<String>,
        event_kind: impl Into<String>,
        scope_key: impl Into<String>,
        data: Value,
    ) -> Self {
        let seq = SEQ_COUNTER.fetch_add(1, Ordering::SeqCst);
        let ts = Utc::now().to_rfc3339();
        let hash = compute_data_hash(&data);

        Self {
            seq,
            ts,
            event_type: event_type.into(),
            event_kind: event_kind.into(),
            scope_key: scope_key.into(),
            data,
            hash,
            agent_id: None,
            device_name: None,
            boot_session_id: None,
        }
    }

    /// Create a new event with full context
    pub fn new_with_context(
        event_type: impl Into<String>,
        event_kind: impl Into<String>,
        scope_key: impl Into<String>,
        data: Value,
        agent_id: impl Into<String>,
        device_name: impl Into<String>,
        boot_session_id: impl Into<String>,
    ) -> Self {
        let mut event = Self::new(event_type, event_kind, scope_key, data);
        event.agent_id = Some(agent_id.into());
        event.device_name = Some(device_name.into());
        event.boot_session_id = Some(boot_session_id.into());
        event
    }

    /// Compute hash of the data payload
    fn compute_data_hash(data: &Value) -> String {
        compute_data_hash(data)
    }
}

/// Compute SHA-256 hash of canonical JSON data for deduplication
pub fn compute_data_hash(data: &Value) -> String {
    // Sort keys for canonical JSON representation
    let canonical = sort_json_keys(data.clone());
    let json_str = serde_json::to_string(&canonical).unwrap_or_default();
    let hash = Sha256::digest(json_str.as_bytes());
    format!("{:x}", hash)
}

/// Recursively sort JSON keys for canonical representation
fn sort_json_keys(value: Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut sorted: serde_json::Map<String, Value> = serde_json::Map::new();
            let mut keys: Vec<_> = map.keys().cloned().collect();
            keys.sort();
            for key in keys {
                if let Some(v) = map.get(&key) {
                    sorted.insert(key, sort_json_keys(v.clone()));
                }
            }
            Value::Object(sorted)
        }
        Value::Array(arr) => {
            Value::Array(arr.into_iter().map(sort_json_keys).collect())
        }
        other => other,
    }
}

/// Configuration for event generation
#[derive(Debug, Clone)]
pub struct EventConfig {
    /// Debounce duration for noisy events (default: 30 seconds)
    pub debounce_secs: u64,
    /// Minimum interval between identical events (dedupe window)
    pub dedupe_window_secs: u64,
    /// Agent ID for context
    pub agent_id: String,
    /// Device name for context
    pub device_name: String,
    /// Boot session ID for context
    pub boot_session_id: String,
}

impl Default for EventConfig {
    fn default() -> Self {
        Self {
            debounce_secs: 30,
            dedupe_window_secs: 60,
            agent_id: String::new(),
            device_name: String::new(),
            boot_session_id: String::new(),
        }
    }
}

/// Helper to create scope keys
pub fn make_scope_key(prefix: &str, identifier: &str) -> String {
    format!("{}:{}", prefix, identifier)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_event_creation() {
        let data = json!({"test": "value"});
        let event = Event::new("service", "state_changed", "service:Test", data);

        assert_eq!(event.event_type, "service");
        assert_eq!(event.event_kind, "state_changed");
        assert_eq!(event.scope_key, "service:Test");
        assert!(!event.hash.is_empty());
        assert!(!event.ts.is_empty());
        assert!(event.seq > 0);
    }

    #[test]
    fn test_hash_consistency() {
        let data = json!({"a": 1, "b": 2});
        let hash1 = compute_data_hash(&data);
        let hash2 = compute_data_hash(&data);
        assert_eq!(hash1, hash2);

        // Different order should produce same hash (canonical)
        let data_reordered = json!({"b": 2, "a": 1});
        let hash3 = compute_data_hash(&data_reordered);
        assert_eq!(hash1, hash3);
    }

    #[test]
    fn test_scope_key() {
        assert_eq!(make_scope_key("service", "Spooler"), "service:Spooler");
        assert_eq!(make_scope_key("software", "Chrome"), "software:Chrome");
    }

    #[test]
    fn test_event_with_context() {
        let data = json!({"test": "value"});
        let event = Event::new_with_context(
            "system",
            "boot",
            "system:boot",
            data,
            "agent-123",
            "DESKTOP-ABC",
            "boot-session-456",
        );

        assert_eq!(event.agent_id, Some("agent-123".to_string()));
        assert_eq!(event.device_name, Some("DESKTOP-ABC".to_string()));
        assert_eq!(event.boot_session_id, Some("boot-session-456".to_string()));
    }
}
