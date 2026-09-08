//! Recurring schedule policy payloads.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProjectSchedulePolicyQuery {
    pub project_id: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProjectSchedulePolicySetRequest {
    pub mutation: crate::MutationContext,
    pub project_id: u64,
    pub catch_up_one: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProjectSchedulePolicyRecord {
    pub project_id: u64,
    pub catch_up_one: bool,
    pub version: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ScheduleAttentionListQuery {
    pub project_id: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ScheduleAttentionReason {
    MissedWindow,
    Overlap,
    Blocked,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ScheduleAttentionRecord {
    pub id: u64,
    pub project_id: u64,
    pub template_ticket_id: u64,
    pub schedule_id: u64,
    pub reason: ScheduleAttentionReason,
    pub first_window: String,
    pub last_window: String,
    pub next_activation: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ScheduleAttentionListResponse {
    pub signals: Vec<ScheduleAttentionRecord>,
}
