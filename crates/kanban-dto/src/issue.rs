//! Linear issue operator payloads (MCP parity).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::mutation::MutationContext;

/// Create or update one issue (`save_issue`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct IssueSaveRequest {
    pub mutation: MutationContext,
    /// Target project when creating; required without `identifier`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<u64>,
    /// Existing issue key (`CODE-123`) when updating.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identifier: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description_patch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority: Option<u8>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::nullable_patch::deserialize_string_patch"
    )]
    pub due_date: Option<Option<String>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::nullable_patch::deserialize_u64_patch"
    )]
    pub milestone_id: Option<Option<u64>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub add_labels: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remove_labels: Option<Vec<String>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::nullable_patch::deserialize_u64_patch"
    )]
    pub parent_id: Option<Option<u64>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::nullable_patch::deserialize_string_patch"
    )]
    pub duplicate_of: Option<Option<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sort_order: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub add_blocks: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remove_blocks: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub add_related: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remove_related: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct IssueSaveResponse {
    pub identifier: String,
    pub url: String,
    pub ticket_id: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct IssueGetQuery {
    pub identifier: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct IssueRelationRecord {
    pub kind: String,
    pub identifier: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct IssueStateHistoryRecord {
    pub state: String,
    pub state_type: String,
    pub started_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct IssueChildRecord {
    pub identifier: String,
    pub title: String,
    pub state: String,
    pub sort_order: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct IssueRecord {
    pub identifier: String,
    pub url: String,
    pub ticket_id: u64,
    pub project_id: u64,
    pub title: String,
    pub description: String,
    pub state: String,
    pub state_type: String,
    pub priority: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub due_date: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub milestone_id: Option<u64>,
    pub labels: Vec<String>,
    pub relations: Vec<IssueRelationRecord>,
    pub state_history: Vec<IssueStateHistoryRecord>,
    pub version: u64,
    pub created_at: String,
    pub updated_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duplicate_of: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duplicates: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub children: Option<Vec<IssueChildRecord>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct IssueListQuery {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_since: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<u64>,
    #[serde(default)]
    pub limit: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct IssueListResponse {
    pub issues: Vec<IssueRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct IssueWorkflowStateRecord {
    pub name: String,
    pub state_type: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct IssueWorkflowStatesQuery {
    pub project_id: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct IssueWorkflowStatesResponse {
    pub states: Vec<IssueWorkflowStateRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct IssueLabelRecord {
    pub name: String,
    pub color: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    #[serde(default)]
    pub retired: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct IssueLabelsQuery {
    pub project_id: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct IssueLabelsResponse {
    pub labels: Vec<IssueLabelRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct IssueMilestoneRecord {
    pub id: u64,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_date: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct IssueMilestonesQuery {
    pub project_id: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct IssueMilestonesResponse {
    pub milestones: Vec<IssueMilestoneRecord>,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::IssueSaveRequest;
    use crate::mutation::MutationContext;

    fn mutation() -> MutationContext {
        MutationContext {
            idempotency_key: "test".into(),
            optimistic_version: 1,
        }
    }

    #[test]
    fn null_due_date_deserializes_as_explicit_clear() {
        let request: IssueSaveRequest = serde_json::from_value(json!({
            "mutation": mutation(),
            "identifier": "CORE-1",
            "due_date": null,
        }))
        .expect("payload parses");
        assert_eq!(request.due_date.as_ref(), Some(&None));
    }
}
