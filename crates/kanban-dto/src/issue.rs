//! Linear issue operator payloads (MCP parity).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::mutation::MutationContext;

/// Create or update one issue (`save_issue`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct IssueSaveRequest {
    pub mutation: MutationContext,
    /// Target project when creating. Required without `identifier`
    /// unless `parent_id` names a parent, whose project it then
    /// defaults to; a sub-issue may name a project other than its
    /// parent's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<u64>,
    /// Kanban project code, the Linear team. For example `CORE`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub team: Option<String>,
    /// Label name in the single-select Project group. This is not the
    /// Kanban project.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    /// Milestone name within the team.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub milestone: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description_edits: Option<Vec<DescriptionEdit>>,
    /// Existing issue key (`CODE-123`) when updating. Also accepted as `id`.
    #[serde(default, skip_serializing_if = "Option::is_none", alias = "id")]
    pub identifier: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(schema_with = "description_patch_schema")]
    pub description_patch: Option<String>,
    /// Kanban workflow state. Valid names: Draft, Parked, Blocked,
    /// Scheduled, Ready, Active, In Review, Approved, Landing, Done,
    /// Cancelled, Superseded, and Duplicate. Linear names such as Todo
    /// are refused.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority: Option<u8>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        alias = "dueDate",
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
    /// Parent issue by key (`CODE-123`) or numeric ticket id; `null`
    /// makes the issue top-level. The parent may be in another
    /// project. A new sub-issue takes the parent's milestone only when
    /// it stays in the parent's project.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        alias = "parentId",
        deserialize_with = "crate::nullable_patch::deserialize_parent_patch"
    )]
    pub parent_id: Option<Option<String>>,
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
    /// Who created the issue. The operator session fills this from the
    /// configured operator name when it is omitted on create.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub creator: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none", alias = "blockedBy")]
    pub add_blocked_by: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remove_blocked_by: Option<Vec<String>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::nullable_patch::deserialize_string_patch"
    )]
    pub assignee: Option<Option<String>>,
}

/// One description edit in a save. Anchors must match exactly once.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DescriptionEdit {
    /// `replace`, `insert_before`, `insert_after`, `prepend`, `append`,
    /// or `replace_range`.
    #[serde(rename = "type")]
    pub op: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub find: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replace: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

/// The runtime accepts `description_patch` as a JSON-encoded object
/// with string `find` and `replace` fields, not free text: the first
/// occurrence of `find` in the description is replaced.
fn description_patch_schema(
    _generator: &mut schemars::r#gen::SchemaGenerator,
) -> schemars::schema::Schema {
    use schemars::schema::{InstanceType, Metadata, SchemaObject, SingleOrVec};
    SchemaObject {
        metadata: Some(Box::new(Metadata {
            description: Some(
                "A JSON-encoded object with string `find` and `replace` fields, \
                 like `{\"find\":\"old text\",\"replace\":\"new text\"}`; \
                 the first occurrence of `find` in the description is replaced."
                    .to_owned(),
            ),
            ..Default::default()
        })),
        instance_type: Some(SingleOrVec::Vec(vec![
            InstanceType::String,
            InstanceType::Null,
        ])),
        ..Default::default()
    }
    .into()
}

/// `parent_id` on a list accepts an issue key, a numeric ticket id, or
/// JSON `null`. The runtime already deserialises the number; the
/// published schema has to say so.
fn parent_filter_schema(
    _generator: &mut schemars::r#gen::SchemaGenerator,
) -> schemars::schema::Schema {
    use schemars::schema::{InstanceType, Metadata, SchemaObject, SingleOrVec};
    SchemaObject {
        metadata: Some(Box::new(Metadata {
            description: Some(
                "Parent issue by key (`CODE-123`) or numeric ticket id, such as 12. \
                 JSON null clears the filter."
                    .to_owned(),
            ),
            ..Default::default()
        })),
        instance_type: Some(SingleOrVec::Vec(vec![
            InstanceType::String,
            InstanceType::Integer,
            InstanceType::Null,
        ])),
        ..Default::default()
    }
    .into()
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
    #[serde(alias = "id")]
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
    pub sort_order: f64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct IssueSubIssueProgress {
    pub done: u32,
    pub total: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct IssueRecord {
    pub identifier: String,
    pub url: String,
    pub ticket_id: u64,
    pub project_id: u64,
    pub title: String,
    #[serde(default)]
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
    #[serde(default)]
    pub state_history: Vec<IssueStateHistoryRecord>,
    pub version: u64,
    pub created_at: String,
    pub updated_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub canceled_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sub_issues: Option<IssueSubIssueProgress>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duplicate_of: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duplicates: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub children: Option<Vec<IssueChildRecord>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assignee: Option<String>,
    /// Kanban project code, the Linear team.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub team: Option<String>,
    /// The issue's label in the single-select Project group.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    /// Milestone name within the team.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub milestone: Option<String>,
    /// Who created the issue, when that was recorded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub creator: Option<String>,
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
    /// Parent issue by key (`CODE-123`) or numeric ticket id. JSON
    /// `null` clears the filter.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        alias = "parentId",
        deserialize_with = "crate::nullable_patch::deserialize_optional_parent_filter"
    )]
    #[schemars(schema_with = "parent_filter_schema")]
    pub parent_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub milestone_id: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub team_id: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assignee: Option<String>,
    /// Kanban project code, the Linear team. For example `CORE`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub team: Option<String>,
    /// Project-group label name. This is not the Kanban project id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    /// Extra record fields to include. `description` and `history`
    /// are omitted unless named here. Unrecognised names are ignored.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fields: Option<Vec<String>>,
    #[serde(default)]
    pub limit: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(
        description = "Opaque page boundary. A numeric ticket id is still accepted. A newly issued cursor also carries the update stamp it was issued against."
    )]
    pub cursor: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct IssueListResponse {
    pub issues: Vec<IssueRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(
        description = "Opaque page boundary for the next page. A numeric ticket id is still accepted. A newly issued cursor also carries the update stamp it was issued against."
    )]
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
    pub group_single_select: bool,
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
    pub issues_total: u32,
    pub issues_done: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct IssueProjectRecord {
    pub id: u64,
    pub code: String,
    pub name: String,
    /// Whether an open parent closes once every sub-issue is closed.
    pub auto_close_sub_issues: bool,
    /// Whether a parent's open sub-issues close when the parent closes.
    pub close_sub_issues_with_parent: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct IssueProjectsQuery {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct IssueProjectsResponse {
    pub projects: Vec<IssueProjectRecord>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct IssueCreateMilestoneRequest {
    pub mutation: MutationContext,
    pub project_id: u64,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_date: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct IssueCreateLabelRequest {
    pub mutation: MutationContext,
    pub project_id: u64,
    pub name: String,
    pub color: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    #[serde(default)]
    pub group_single_select: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct IssueRetireLabelRequest {
    pub mutation: MutationContext,
    pub project_id: u64,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct IssueUpdateProjectSettingsRequest {
    pub mutation: MutationContext,
    pub project_id: u64,
    /// Close an open parent automatically once every sub-issue is
    /// closed (Linear's "auto-close parent issues").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_close_sub_issues: Option<bool>,
    /// Close a parent's open sub-issues when the parent closes
    /// (Linear's "auto-close sub-issues"): a completed parent completes
    /// them, and a canceled or duplicate parent cancels them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub close_sub_issues_with_parent: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct IssueProjectSettingsResponse {
    pub project_id: u64,
    pub auto_close_sub_issues: bool,
    pub close_sub_issues_with_parent: bool,
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

    #[test]
    fn list_query_schema_accepts_a_numeric_parent_id() {
        let schema = schemars::schema_for!(super::IssueListQuery);
        let encoded = serde_json::to_value(&schema).expect("the schema serialises");
        let parent = &encoded["properties"]["parent_id"];
        let accepts_integer = parent["type"]
            .as_array()
            .is_some_and(|kinds| kinds.iter().any(|kind| kind == "integer"))
            || parent["anyOf"]
                .as_array()
                .is_some_and(|options| options.iter().any(|option| option["type"] == "integer"));
        assert!(
            accepts_integer,
            "parent_id must accept a numeric ticket id: {parent}"
        );

        let published = std::fs::read_to_string(format!(
            "{}/../../packages/contracts/src/schemas/IssueListQuery.json",
            env!("CARGO_MANIFEST_DIR")
        ))
        .expect("the published list query schema reads");
        let published: serde_json::Value =
            serde_json::from_str(&published).expect("the published schema is JSON");
        let published_parent = &published["properties"]["parent_id"];
        let published_accepts = published_parent["type"]
            .as_array()
            .is_some_and(|kinds| kinds.iter().any(|kind| kind == "integer"))
            || published_parent["anyOf"]
                .as_array()
                .is_some_and(|options| options.iter().any(|option| option["type"] == "integer"));
        assert!(
            published_accepts,
            "the published list_issues schema must accept {{\"parent_id\": 12}}: {published_parent}"
        );

        let tools = std::fs::read_to_string(format!(
            "{}/../../packages/contracts/src/mcp-tools.json",
            env!("CARGO_MANIFEST_DIR")
        ))
        .expect("the published MCP tools read");
        let tools: serde_json::Value =
            serde_json::from_str(&tools).expect("the published tools are JSON");
        let list_issues = tools
            .as_array()
            .expect("tools are a list")
            .iter()
            .find(|tool| tool["name"] == "list_issues")
            .expect("list_issues is published");
        let listed = &list_issues["inputSchema"]["properties"]["parent_id"];
        let listed_accepts = listed["type"]
            .as_array()
            .is_some_and(|kinds| kinds.iter().any(|kind| kind == "integer"))
            || listed["anyOf"]
                .as_array()
                .is_some_and(|options| options.iter().any(|option| option["type"] == "integer"));
        assert!(
            listed_accepts,
            "the list_issues input schema must accept {{\"parent_id\": 12}}: {listed}"
        );
    }
}
