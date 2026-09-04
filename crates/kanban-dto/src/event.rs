use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Ordered event frame streamed to every transport subscriber.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EventEnvelope {
    pub sequence: u64,
    pub event_type: String,
    pub payload: Value,
}
