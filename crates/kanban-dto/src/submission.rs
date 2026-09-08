//! Structured results, distinct from observed agent output.
use crate::{CapabilityRole, MutationContext};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SubmissionResult {
    Implementation {
        tip: String,
        summary: String,
    },
    Review {
        tip: String,
        summary: String,
        approve: bool,
        #[serde(default)]
        findings: Vec<crate::ReviewFindingRecord>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SubmissionSubmitRequest {
    pub mutation: MutationContext,
    pub run_id: u64,
    pub capability_id: u64,
    pub result: SubmissionResult,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SubmissionRecord {
    pub id: u64,
    pub project_id: u64,
    pub ticket_id: u64,
    pub run_id: u64,
    pub capability_id: u64,
    pub role: CapabilityRole,
    pub reviewer_slot_id: Option<u64>,
    pub result: SubmissionResult,
    pub created_at: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SubmissionListQuery {
    pub project_id: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SubmissionListResponse {
    pub project_id: u64,
    pub submissions: Vec<SubmissionRecord>,
}
