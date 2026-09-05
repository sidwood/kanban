//! Deferral payload definitions: immutable records that may only be
//! superseded explicitly (KAN-S2-US3, DR-AE-03).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// One immutable deferral as every client sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DeferralRecord {
    /// The storage-assigned identity.
    pub id: u64,
    /// The project the deferral belongs to.
    pub project_id: u64,
    /// The finding that was deferred.
    pub finding_id: String,
    /// Why the finding was deferred.
    pub reason: String,
    /// The deferral this one supersedes, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supersedes_id: Option<u64>,
    /// When the deferral was recorded.
    pub recorded_at: String,
}

/// Request payload for the `deferral.record` command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DeferralRecordRequest {
    pub mutation: super::MutationContext,
    pub project_id: u64,
    pub finding_id: String,
    pub reason: String,
}

/// Request payload for the `deferral.supersede` command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DeferralSupersedeRequest {
    pub mutation: super::MutationContext,
    pub project_id: u64,
    pub deferral_id: u64,
    pub reason: String,
}

/// Request payload for the `deferral.list` query.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DeferralListQuery {
    pub project_id: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finding_id: Option<String>,
}

/// Response payload for the `deferral.list` query.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DeferralListResponse {
    /// Every deferral for the project, superseded originals included.
    pub deferrals: Vec<DeferralRecord>,
}
