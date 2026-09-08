//! Guarded landing payloads: Ticket Lanes land into a Spec
//! integration branch, a final integration review lands through the
//! Seed, and standalone Bugs land through the Seed when no active
//! Spec is attached.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SpecIntegrationClaimRequest {
    pub mutation: super::MutationContext,
    pub spec_id: u64,
    pub branch: String,
    pub workspace_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SpecIntegrationApproveRequest {
    pub mutation: super::MutationContext,
    pub spec_id: u64,
    pub reviewed_tip: String,
    pub reviewer: String,
    pub evidence: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SpecIntegrationRecord {
    pub spec_id: u64,
    pub branch: String,
    pub workspace_path: String,
    pub workspace_id: Option<u64>,
    pub review_approved: bool,
    pub approved_tip: Option<String>,
    pub base_tip: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LandingLaneRequest {
    pub mutation: super::MutationContext,
    pub project_id: u64,
    pub spec_id: u64,
    pub from_path: String,
    pub into_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LandingSeedRequest {
    pub mutation: super::MutationContext,
    pub project_id: u64,
    pub spec_id: u64,
    pub from_path: String,
    pub into_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LandingBugRequest {
    pub mutation: super::MutationContext,
    pub project_id: u64,
    pub ticket_id: u64,
    pub from_path: String,
    pub into_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LandingRecord {
    pub id: u64,
    pub project_id: u64,
    pub kind: String,
    pub from_path: String,
    pub into_path: String,
    pub from_branch: String,
    pub into_branch: String,
    pub from_tip: String,
    pub into_tip: String,
    pub landed_tip: String,
    pub spec_id: Option<u64>,
    pub ticket_id: Option<u64>,
}
