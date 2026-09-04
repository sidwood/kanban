use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Request payload for the `health.get` query.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HealthQuery {}

/// Response payload for the `health.get` query.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HealthResponse {
    pub connected: bool,
    pub service_version: String,
}
