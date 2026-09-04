use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Metadata every mutation must carry so retries and stale edits are safe.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MutationContext {
    pub optimistic_version: u64,
    pub idempotency_key: String,
}
