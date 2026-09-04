use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The closed vocabulary of timeline event kinds (DR-AE-01).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TimelineEventKind {
    Transition,
    Run,
    Telemetry,
    Review,
    Finding,
    Evidence,
    Comment,
    Deferral,
    Ruling,
}

/// Entity kinds that may appear on the activity timeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TimelineEntityKind {
    Initiative,
    Project,
    Plan,
    Spec,
    Ticket,
    Run,
    Review,
    Finding,
    Evidence,
    Comment,
}

/// A timeline-visible entity reference.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TimelineEntityRef {
    pub kind: TimelineEntityKind,
    pub id: String,
}

/// One append-only timeline row as returned by queries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TimelineEventRecord {
    pub id: u64,
    pub project_id: String,
    pub kind: TimelineEventKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entity: Option<TimelineEntityRef>,
    pub recorded_at: String,
    pub detail: Value,
}

/// Filters for the per-Project timeline query surface.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TimelineQuery {
    pub project_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entity: Option<TimelineEntityRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kinds: Option<Vec<TimelineEventKind>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub until: Option<String>,
}

/// The timeline query answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TimelineQueryResponse {
    pub events: Vec<TimelineEventRecord>,
}
