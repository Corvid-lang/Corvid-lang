use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConnectorTraceEvent {
    pub connector: String,
    pub operation: String,
    pub tenant_id: String,
    pub actor_id: String,
    pub mode: String,
    pub status: String,
    pub scope: String,
    pub effect_ids: Vec<String>,
    pub data_classes: Vec<String>,
    pub replay_key: String,
    pub latency_ms: u64,
    pub redacted: bool,
}
