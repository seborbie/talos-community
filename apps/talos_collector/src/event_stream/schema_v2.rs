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
