//! Executed reviews freeze configuration and profile snapshots.
use crate::{
    MutationContext, ProfileSnapshotRecord, TicketReviewOccupant, TicketReviewSlotRequirement,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ReviewExecutionStatus {
    InProgress,
    Approved,
    Rejected,
    Expired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ReviewStageStatus {
    Waiting,
    Approved,
    Rejected,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReviewStartRequest {
    pub mutation: MutationContext,
    pub ticket_id: u64,
    pub submission_id: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReviewRevalidateRequest {
    pub mutation: MutationContext,
    pub ticket_id: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReviewExpireRequest {
    pub mutation: MutationContext,
    pub review_id: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReviewHistoryQuery {
    pub ticket_id: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReviewAttemptRecord {
    pub attempt: u32,
    pub review_id: Option<u64>,
    pub outcome: String,
    pub verdicts: Vec<String>,
    pub invalidations: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReviewHistoryResponse {
    pub needs_revalidation: bool,
    pub attempts: Vec<ReviewAttemptRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReviewGetQuery {
    pub review_id: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReviewHumanSubmitRequest {
    pub mutation: MutationContext,
    pub review_id: u64,
    pub slot_id: u64,
    pub tip: String,
    pub approve: bool,
    pub summary: String,
    #[serde(default)]
    pub findings: Vec<crate::ReviewFindingRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReviewVerdictRecord {
    /// Late optional results remain evidence but cannot rewrite a resolved stage.
    pub counts_for_resolution: bool,
    pub submission_id: Option<u64>,
    pub tip: String,
    pub approve: bool,
    pub summary: String,
    #[serde(default)]
    pub findings: Vec<crate::ReviewFindingRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReviewerDispatchRecord {
    pub review_id: u64,
    pub slot_id: u64,
    pub tip: String,
    pub requested: ProfileSnapshotRecord,
    pub effective: ProfileSnapshotRecord,
    pub fallback_path: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReviewSlotRecord {
    pub id: u64,
    pub requirement: TicketReviewSlotRequirement,
    pub occupant: TicketReviewOccupant,
    pub requested: Option<ProfileSnapshotRecord>,
    pub effective: Option<ProfileSnapshotRecord>,
    pub fallback_path: Vec<String>,
    pub dispatch_request_id: Option<u64>,
    pub verdict: Option<ReviewVerdictRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReviewStageRecord {
    pub index: usize,
    pub status: ReviewStageStatus,
    pub slots: Vec<ReviewSlotRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReviewExecutionRecord {
    pub id: u64,
    pub project_id: u64,
    pub ticket_id: u64,
    pub submission_id: u64,
    pub tip: String,
    pub configuration_version: u64,
    pub version: u64,
    pub status: ReviewExecutionStatus,
    pub stages: Vec<ReviewStageRecord>,
    pub bounce: Option<ReviewBounceRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReviewFindingReference {
    pub slot_id: u64,
    pub submission_id: Option<u64>,
    pub finding_index: u64,
    pub finding: crate::ReviewFindingRecord,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReviewBounceRecord {
    pub stage_index: usize,
    pub tip: String,
    pub findings: Vec<ReviewFindingReference>,
}
