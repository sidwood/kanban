//! Workspace payload definitions: registration, observation, health,
//! and list surfaces (KAN-S6-US1).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The closed health vocabulary as every client sees it. The
/// `unobserved` state means the last git status read could not
/// complete: the tree claims neither clean nor dirty (KAN-T99).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceHealthDto {
    Available,
    Assigned,
    Dirty,
    Missing,
    Retired,
    Unobserved,
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
            Self::Unobserved => "unobserved",
        }
    }
}

/// The closed checkout vocabulary as every client sees it: HEAD is
/// on a branch, or detached. Detached is a state of its own, never a
/// branch name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceCheckoutDto {
    Branch,
    Detached,
}

impl WorkspaceCheckoutDto {
    /// The wire name, matching this variant's serialised form.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Branch => "branch",
            Self::Detached => "detached",
        }
    }
}

/// The git facts the core last observed without mutating the clone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceObservationDto {
    pub repository_identity: Option<String>,
    pub checkout: Option<WorkspaceCheckoutDto>,
    pub branch: Option<String>,
    pub head: Option<String>,
    /// Whether the working tree is clean. Absent when no status read
    /// completed — a failed observation, never a clean verdict.
    pub working_tree_clean: Option<bool>,
    /// Whether the Workspace holds unique unlanded commits; absent
    /// when the observer could not decide.
    pub unique_unlanded_commits: Option<bool>,
    pub lane_assignment: Option<u64>,
}

/// The reuse verdict with every condition evaluated (DR-LW-06).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceReuseDto {
    /// Reusable only when every condition holds and the record is
    /// present and not retired.
    pub reusable: bool,
    /// The working tree is clean on a present path.
    pub clean: bool,
    /// No Lane assignment holds the Workspace.
    pub unassigned: bool,
    /// No unique unlanded commits sit on the Workspace.
    pub free_of_unlanded_commits: bool,
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
    pub reuse: WorkspaceReuseDto,
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

/// Request payload for the `workspace.retire` command. Retirement is
/// the explicit operator action that ends reuse; the record is
/// preserved, never deleted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceRetireRequest {
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
        WorkspaceCheckoutDto, WorkspaceHealthDto, WorkspaceListQuery, WorkspaceObserveRequest,
        WorkspaceRecord, WorkspaceRegisterRequest, WorkspaceRetireRequest,
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
    fn workspace_retire_request_rejects_unknown_fields() {
        let payload = json!({
            "mutation": context(),
            "workspace_id": 1,
            "reason": "stale",
        });

        let error = serde_json::from_value::<WorkspaceRetireRequest>(payload)
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
                checkout: Some(WorkspaceCheckoutDto::Branch),
                branch: Some("main".to_owned()),
                head: Some("abc".to_owned()),
                working_tree_clean: Some(true),
                unique_unlanded_commits: Some(false),
                lane_assignment: None,
            },
            reuse: super::WorkspaceReuseDto {
                reusable: true,
                clean: true,
                unassigned: true,
                free_of_unlanded_commits: true,
            },
            version: 3,
        };

        let encoded = serde_json::to_value(&record).expect("the record encodes");
        let decoded: WorkspaceRecord = serde_json::from_value(encoded).expect("the record decodes");

        assert_eq!(decoded, record);
    }

    #[test]
    fn workspace_record_round_trips_a_detached_checkout() {
        let record = WorkspaceRecord {
            id: 1,
            project_id: 2,
            path: "/workspaces/core".to_owned(),
            is_seed: false,
            health: WorkspaceHealthDto::Available,
            observation: super::WorkspaceObservationDto {
                repository_identity: Some("identity".to_owned()),
                checkout: Some(WorkspaceCheckoutDto::Detached),
                branch: None,
                head: Some("abc".to_owned()),
                working_tree_clean: Some(true),
                unique_unlanded_commits: Some(false),
                lane_assignment: None,
            },
            reuse: super::WorkspaceReuseDto {
                reusable: true,
                clean: true,
                unassigned: true,
                free_of_unlanded_commits: true,
            },
            version: 3,
        };

        let encoded = serde_json::to_value(&record).expect("the record encodes");

        assert_eq!(encoded["observation"]["checkout"], json!("detached"));
        assert_eq!(encoded["observation"]["branch"], json!(null));
        let decoded: WorkspaceRecord = serde_json::from_value(encoded).expect("the record decodes");
        assert_eq!(decoded, record);
    }

    #[test]
    fn workspace_checkout_wire_names_round_trip() {
        for checkout in [WorkspaceCheckoutDto::Branch, WorkspaceCheckoutDto::Detached] {
            let wire = serde_json::to_value(checkout).expect("the checkout encodes");
            assert_eq!(wire, json!(checkout.as_str()));
            let decoded: WorkspaceCheckoutDto =
                serde_json::from_value(wire).expect("the checkout decodes");
            assert_eq!(decoded, checkout);
        }
    }

    #[test]
    fn workspace_record_round_trips_an_unobserved_health() {
        let record = WorkspaceRecord {
            id: 1,
            project_id: 2,
            path: "/workspaces/core".to_owned(),
            is_seed: false,
            health: super::WorkspaceHealthDto::Unobserved,
            observation: super::WorkspaceObservationDto {
                repository_identity: Some("identity".to_owned()),
                checkout: Some(WorkspaceCheckoutDto::Branch),
                branch: Some("feature".to_owned()),
                head: Some("abc".to_owned()),
                working_tree_clean: None,
                unique_unlanded_commits: None,
                lane_assignment: None,
            },
            reuse: super::WorkspaceReuseDto {
                reusable: false,
                clean: false,
                unassigned: true,
                free_of_unlanded_commits: false,
            },
            version: 3,
        };

        let encoded = serde_json::to_value(&record).expect("the record encodes");

        assert_eq!(encoded["health"], json!("unobserved"));
        assert_eq!(encoded["observation"]["working_tree_clean"], json!(null));
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
        assert!(names.contains(&"WorkspaceCheckoutDto"));
    }
}
