//! Read-only schedule preview payloads.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SchedulePreviewQuery {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cron: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub activation: Option<String>,
    pub timezone: String,
    pub after: String,
    #[schemars(range(min = 1, max = 20))]
    pub count: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ScheduleDstKind {
    FixedInstant,
    FixedTime,
    IntervalWildcard,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ScheduleDstBehaviour {
    pub kind: ScheduleDstKind,
    pub spring_forward: String,
    pub fall_back: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ScheduleActivationPreview {
    pub utc: String,
    pub local: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SchedulePreviewResponse {
    pub activations: Vec<ScheduleActivationPreview>,
    pub dst_behaviour: ScheduleDstBehaviour,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ScheduleGetQuery {
    pub ticket_id: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ScheduleRecord {
    pub id: u64,
    pub activation: Option<String>,
    pub cron: Option<String>,
    pub timezone: String,
    pub profile: String,
    pub next_activation: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ScheduleGetResponse {
    pub ticket_id: u64,
    pub schedule: Option<ScheduleRecord>,
}
