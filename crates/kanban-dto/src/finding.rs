//! Structured review findings travel with their immutable submission.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum FindingSeverity {
    P0,
    P1,
    P2,
    P3,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReviewFindingRecord {
    pub severity: FindingSeverity,
    pub in_scope: bool,
    pub summary: String,
    pub evidence: String,
    pub location: String,
    pub proposed_resolution: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FindingRecord {
    pub promotion: Option<crate::DeferralPromotionRecord>,
    pub id: String,
    pub project_id: u64,
    pub review_id: u64,
    pub ticket_id: u64,
    pub slot_id: u64,
    pub submission_id: Option<u64>,
    pub tip: String,
    pub counts_for_resolution: bool,
    pub finding: ReviewFindingRecord,
    pub blocking: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FindingListQuery {
    pub project_id: u64,
    #[serde(default)]
    pub review_id: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FindingGetQuery {
    pub project_id: u64,
    pub finding_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FindingListResponse {
    pub project_id: u64,
    pub findings: Vec<FindingRecord>,
}
