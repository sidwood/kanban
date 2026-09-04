use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// A transport-visible failure returned to every client.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ApiError {
    pub code: ErrorCode,
    pub message: String,
}

/// Stable error codes shared by commands, queries, and events.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    UnknownField,
    StaleVersion,
    DuplicateIdempotencyKey,
    NotFound,
    Internal,
}
