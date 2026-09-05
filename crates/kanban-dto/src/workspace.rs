//! Workspace payload definitions: registration, observation, health,
//! and list surfaces (KAN-S6-US1).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The closed health vocabulary as every client sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceHealthDto {
    Available,
    Assigned,
    Dirty,
    Missing,
    Retired,
}

impl WorkspaceHealthDto {
    /// The wire name, matching this variant's serialised form.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Available => "available",
            Self::Assigned => "assigned",
            Self::Dirty => "dirty",
            Self::Missing => "missing",
            Self::Retired => "retired",
        }
    }
}

/// The git facts the core last observed without mutating the clone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceObservationDto {
    pub repository_identity: Option<String>,
    pub branch: Option<String>,
    pub head: Option<String>,
    pub working_tree_clean: Option<bool>,
    pub lane_assignment: Option<u64>,
}

/// The Workspace record as every client sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceRecord {
    pub id: u64,
    pub project_id: u64,
    pub path: String,
    pub is_seed: bool,
    pub health: WorkspaceHealthDto,
    pub observation: WorkspaceObservationDto,
    pub version: u64,
}

/// Request payload for the `workspace.register` command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceRegisterRequest {
    pub mutation: super::MutationContext,
    pub project_id: u64,
    pub path: String,
}

/// Request payload for the `workspace.observe` command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceObserveRequest {
    pub mutation: super::MutationContext,
    pub workspace_id: u64,
}

/// Request payload for the `workspace.list` query.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceListQuery {
    pub project_id: u64,
}

/// Response payload for the `workspace.list` query.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceListResponse {
    pub workspaces: Vec<WorkspaceRecord>,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        WorkspaceHealthDto, WorkspaceListQuery, WorkspaceObserveRequest, WorkspaceRecord,
        WorkspaceRegisterRequest,
    };
    use crate::mutation::MutationContext;
    use crate::schema_definitions;

    fn context() -> MutationContext {
        MutationContext {
            optimistic_version: 0,
            idempotency_key: "key-1".to_owned(),
        }
    }

    #[test]
    fn workspace_register_request_rejects_unknown_fields() {
        let payload = json!({
            "mutation": context(),
            "project_id": 1,
            "path": "/workspaces/core",
            "surprise": true,
        });

        let error = serde_json::from_value::<WorkspaceRegisterRequest>(payload)
            .expect_err("unknown fields are rejected");

        assert!(error.to_string().contains("unknown field"));
    }

    #[test]
    fn workspace_observe_request_rejects_unknown_fields() {
        let payload = json!({
            "mutation": context(),
            "workspace_id": 1,
            "extra": 1,
        });

        let error = serde_json::from_value::<WorkspaceObserveRequest>(payload)
            .expect_err("unknown fields are rejected");

        assert!(error.to_string().contains("unknown field"));
    }

    #[test]
    fn workspace_list_query_rejects_unknown_fields() {
        let payload = json!({ "project_id": 1, "include_retired": true });

        let error = serde_json::from_value::<WorkspaceListQuery>(payload)
            .expect_err("unknown fields are rejected");

        assert!(error.to_string().contains("unknown field"));
    }

    #[test]
    fn workspace_record_round_trips() {
        let record = WorkspaceRecord {
            id: 1,
            project_id: 2,
            path: "/workspaces/core".to_owned(),
            is_seed: true,
            health: WorkspaceHealthDto::Available,
            observation: super::WorkspaceObservationDto {
                repository_identity: Some("identity".to_owned()),
                branch: Some("main".to_owned()),
                head: Some("abc".to_owned()),
                working_tree_clean: Some(true),
                lane_assignment: None,
            },
            version: 3,
        };

        let encoded = serde_json::to_value(&record).expect("the record encodes");
        let decoded: WorkspaceRecord = serde_json::from_value(encoded).expect("the record decodes");

        assert_eq!(decoded, record);
    }

    #[test]
    fn workspace_list_response_is_in_the_schema_registry() {
        let names: Vec<_> = schema_definitions()
            .into_iter()
            .map(|(name, _)| name)
            .collect();

        assert!(names.contains(&"WorkspaceListResponse"));
    }
}
