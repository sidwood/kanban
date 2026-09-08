//! Criterion evidence bindings: attach, review, satisfy, and complete
//! at one reviewed code tip.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CriterionKindDto {
    Acceptance,
    Task,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceReviewDto {
    Pending,
    Validated,
    Rejected,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CriterionBindingRecord {
    pub ticket_id: u64,
    pub criterion_index: u64,
    pub kind: CriterionKindDto,
    pub evidence_id: u64,
    pub tip: String,
    pub review: EvidenceReviewDto,
    pub satisfied: bool,
    pub void: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CriterionEvidenceAttachRequest {
    pub mutation: super::MutationContext,
    pub ticket_id: u64,
    pub criterion_index: u64,
    pub evidence_id: u64,
    pub tip: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CriterionEvidenceReviewRequest {
    pub mutation: super::MutationContext,
    pub ticket_id: u64,
    pub criterion_index: u64,
    pub review: EvidenceReviewDto,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CriterionSatisfyRequest {
    pub mutation: super::MutationContext,
    pub ticket_id: u64,
    pub criterion_index: u64,
    pub tip: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CriterionCompleteRequest {
    pub mutation: super::MutationContext,
    pub ticket_id: u64,
    pub criterion_index: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CriterionInvalidateRequest {
    pub mutation: super::MutationContext,
    pub ticket_id: u64,
    pub observed_tip: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CriterionBindingListQuery {
    pub ticket_id: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CriterionBindingListResponse {
    pub bindings: Vec<CriterionBindingRecord>,
}
