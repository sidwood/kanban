//! Explicit recovery history, separate from the Ticket's verdict.
use crate::MutationContext;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RunRecoveryAction {
    OperatorRuling,
    Retry,
    Resume,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RunRecoveryRuleRequest {
    pub mutation: MutationContext,
    pub run_id: u64,
    pub summary: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RunRecoveryRecord {
    pub id: u64,
    pub run_id: u64,
    pub project_id: u64,
    pub action: RunRecoveryAction,
    pub summary: String,
    pub ruling_id: u64,
    pub replacement_dispatch_request_id: Option<u64>,
    pub created_at: u64,
    pub version: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RunRecoveryListQuery {
    pub run_id: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RunRecoveryResumeRequest {
    pub mutation: MutationContext,
    pub run_id: u64,
    pub summary: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RunRecoveryRetryRequest {
    pub mutation: MutationContext,
    pub run_id: u64,
    pub summary: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RunRecoveryListResponse {
    pub run_id: u64,
    pub version: u64,
    pub can_resume: bool,
    pub can_retry: bool,
    #[serde(default)]
    pub pending_resume: bool,
    pub records: Vec<RunRecoveryRecord>,
}
