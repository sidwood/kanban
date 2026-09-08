//! The global Attention Inbox projection.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AttentionSubjectKind {
    Ticket,
    Run,
    Spec,
    Project,
    Deferral,
    Graph,
    Schedule,
    Role,
}
impl AttentionSubjectKind {
    pub const ALL: &'static [Self] = &[
        Self::Ticket,
        Self::Run,
        Self::Spec,
        Self::Project,
        Self::Deferral,
        Self::Graph,
        Self::Schedule,
        Self::Role,
    ];
    pub fn wire_name(self) -> &'static str {
        match self {
            Self::Ticket => "ticket",
            Self::Run => "run",
            Self::Spec => "spec",
            Self::Project => "project",
            Self::Deferral => "deferral",
            Self::Graph => "graph",
            Self::Schedule => "schedule",
            Self::Role => "role",
        }
    }
    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|kind| kind.wire_name() == value)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AttentionListQuery {
    #[serde(default)]
    pub include_acknowledged: bool,
    #[serde(default)]
    pub include_inactive: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AttentionItemRecord {
    pub id: String,
    pub project_id: u64,
    pub kind: crate::AttentionState,
    pub subject_kind: AttentionSubjectKind,
    pub subject_id: String,
    pub summary: String,
    pub detail: Value,
    pub version: u64,
    pub active: bool,
    pub acknowledged_by: Option<String>,
    pub acknowledged_at: Option<String>,
    pub first_seen_at: String,
    pub last_seen_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AttentionListResponse {
    pub items: Vec<AttentionItemRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AttentionAcknowledgeRequest {
    pub mutation: crate::MutationContext,
    pub item_id: String,
    pub who: String,
}
