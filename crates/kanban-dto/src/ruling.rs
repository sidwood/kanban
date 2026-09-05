//! Ruling payload definitions: immutable records that may only be
//! superseded explicitly (KAN-S2-US3, DR-AE-03).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::timeline::TimelineEntityRef;

/// One immutable ruling as every client sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RulingRecord {
    /// The storage-assigned identity.
    pub id: u64,
    /// The project the ruling belongs to.
    pub project_id: u64,
    /// The entity the ruling concerns, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entity: Option<TimelineEntityRef>,
    /// The operator decision text.
    pub summary: String,
    /// The ruling this one supersedes, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supersedes_id: Option<u64>,
    /// When the ruling was recorded.
    pub recorded_at: String,
}

/// Request payload for the `ruling.record` command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RulingRecordRequest {
    pub mutation: super::MutationContext,
    pub project_id: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entity: Option<TimelineEntityRef>,
    pub summary: String,
}

/// Request payload for the `ruling.supersede` command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RulingSupersedeRequest {
    pub mutation: super::MutationContext,
    pub project_id: u64,
    pub ruling_id: u64,
    pub summary: String,
}

/// Request payload for the `ruling.list` query.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RulingListQuery {
    pub project_id: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entity: Option<TimelineEntityRef>,
}

/// Response payload for the `ruling.list` query.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RulingListResponse {
    /// Every ruling for the project, superseded originals included.
    pub rulings: Vec<RulingRecord>,
}
