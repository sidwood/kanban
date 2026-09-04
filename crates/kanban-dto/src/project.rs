//! Project payload definitions: the record every client sees and the
//! register, archive, and list payloads (KAN-S1-US4, KAN-S1-US5,
//! KAN-S1-US6). There is deliberately no delete payload and no way
//! to change a code: codes are minted once and never change.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The Plan, Spec, and Ticket counters of one Project: the last
/// number minted per kind. Zero means nothing has been minted yet,
/// and the counters survive archiving.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProjectCounters {
    /// The last Plan number minted.
    pub plan: u64,
    /// The last Spec number minted.
    pub spec: u64,
    /// The last Ticket number minted.
    pub ticket: u64,
}

/// The Project record as every client sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProjectRecord {
    /// The immutable, storage-assigned identity.
    pub id: u64,
    /// The immutable, globally unique Project code.
    pub code: String,
    /// The display name.
    pub name: String,
    /// The one target Git repository.
    pub repository: String,
    /// The one Seed Workspace.
    pub seed_workspace: String,
    /// The one default branch.
    pub default_branch: String,
    /// The one exclusive named Herdr session.
    pub herdr_session: String,
    /// The Initiative the Project sits under, if any.
    pub initiative_id: Option<u64>,
    /// Whether the Project is archived. Archived Projects stay
    /// listed; no fact is ever removed.
    pub archived: bool,
    /// The Project's counters, preserved through every state.
    pub counters: ProjectCounters,
    /// The aggregate version, for optimistic mutation checks.
    pub version: u64,
}

/// Request payload for the `project.register` command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProjectRegisterRequest {
    pub mutation: super::MutationContext,
    /// The intended code; it must match `[A-Z][A-Z0-9]{1,7}` in full
    /// and `KAN` is reserved.
    pub code: String,
    /// The intended name; blank names are refused.
    pub name: String,
    /// The target Git repository; non-Git targets are refused.
    pub repository: String,
    /// The Seed Workspace.
    pub seed_workspace: String,
    /// The default branch.
    pub default_branch: String,
    /// The exclusive Herdr session name; duplicate session names are
    /// refused.
    pub herdr_session: String,
    /// The Initiative the Project sits under, if any.
    pub initiative_id: Option<u64>,
}

/// Request payload for the `project.archive` command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProjectArchiveRequest {
    pub mutation: super::MutationContext,
    /// The Project being archived.
    pub project_id: u64,
}

/// Request payload for the `project.list` query.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProjectListQuery {}

/// Response payload for the `project.list` query.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProjectListResponse {
    /// Every Project, archived included, newest last.
    pub projects: Vec<ProjectRecord>,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        ProjectArchiveRequest, ProjectCounters, ProjectListQuery, ProjectListResponse,
        ProjectRecord, ProjectRegisterRequest,
    };
    use crate::mutation::MutationContext;
    use crate::schema_definitions;

    fn context() -> MutationContext {
        MutationContext {
            optimistic_version: 0,
            idempotency_key: "key-1".to_owned(),
        }
    }

    fn record(initiative_id: Option<u64>) -> ProjectRecord {
        ProjectRecord {
            id: 3,
            code: "CORE".to_owned(),
            name: "Control plane".to_owned(),
            repository: "/repositories/kanban".to_owned(),
            seed_workspace: "/workspaces/kanban.seed".to_owned(),
            default_branch: "main".to_owned(),
            herdr_session: "kanban-main".to_owned(),
            initiative_id,
            archived: false,
            counters: ProjectCounters {
                plan: 1,
                spec: 0,
                ticket: 4,
            },
            version: 2,
        }
    }

    #[test]
    fn a_record_round_trips() {
        let encoded = serde_json::to_value(record(None)).expect("the record serialises");
        assert_eq!(
            encoded,
            json!({
                "id": 3,
                "code": "CORE",
                "name": "Control plane",
                "repository": "/repositories/kanban",
                "seed_workspace": "/workspaces/kanban.seed",
                "default_branch": "main",
                "herdr_session": "kanban-main",
                "initiative_id": null,
                "archived": false,
                "counters": { "plan": 1, "spec": 0, "ticket": 4 },
                "version": 2,
            })
        );
        let decoded: ProjectRecord =
            serde_json::from_value(encoded).expect("the record deserialises");
        assert_eq!(decoded, record(None));
    }

    #[test]
    fn a_record_carries_its_initiative_when_it_has_one() {
        let encoded = serde_json::to_value(record(Some(7))).expect("the record serialises");

        assert_eq!(encoded["initiative_id"], json!(7));
    }

    #[test]
    fn a_register_request_round_trips_and_rejects_unknown_fields() {
        let request = ProjectRegisterRequest {
            mutation: context(),
            code: "CORE".to_owned(),
            name: "Control plane".to_owned(),
            repository: "/repositories/kanban".to_owned(),
            seed_workspace: "/workspaces/kanban.seed".to_owned(),
            default_branch: "main".to_owned(),
            herdr_session: "kanban-main".to_owned(),
            initiative_id: Some(2),
        };

        let encoded = serde_json::to_value(&request).expect("the request serialises");
        let decoded: ProjectRegisterRequest =
            serde_json::from_value(encoded).expect("the request deserialises");
        assert_eq!(decoded, request);

        let refused: Result<ProjectRegisterRequest, _> = serde_json::from_value(json!({
            "mutation": context(),
            "code": "CORE",
            "name": "Control plane",
            "repository": "/repositories/kanban",
            "seed_workspace": "/workspaces/kanban.seed",
            "default_branch": "main",
            "herdr_session": "kanban-main",
            "initiative_id": null,
            "delete": true,
        }));
        assert!(refused.is_err(), "unknown fields are rejected");
    }

    #[test]
    fn a_register_request_leaves_the_initiative_out_when_unnamed() {
        let decoded: ProjectRegisterRequest = serde_json::from_value(json!({
            "mutation": context(),
            "code": "CORE",
            "name": "Control plane",
            "repository": "/repositories/kanban",
            "seed_workspace": "/workspaces/kanban.seed",
            "default_branch": "main",
            "herdr_session": "kanban-main",
        }))
        .expect("the initiative is optional");

        assert_eq!(decoded.initiative_id, None);
    }

    #[test]
    fn an_archive_request_holds_the_identity_and_context_only() {
        let request = ProjectArchiveRequest {
            mutation: context(),
            project_id: 5,
        };

        let encoded = serde_json::to_value(&request).expect("the request serialises");
        assert_eq!(
            encoded,
            json!({
                "mutation": { "optimistic_version": 0, "idempotency_key": "key-1" },
                "project_id": 5
            })
        );
    }

    #[test]
    fn a_list_response_carries_its_records() {
        let response = ProjectListResponse {
            projects: vec![record(None), record(Some(1))],
        };

        let encoded = serde_json::to_value(&response).expect("the response serialises");
        assert_eq!(encoded["projects"].as_array().map(Vec::len), Some(2));
        let decoded: ProjectListResponse =
            serde_json::from_value(encoded).expect("the response deserialises");
        assert_eq!(decoded, response);
    }

    #[test]
    fn every_project_schema_rejects_unknown_fields() {
        for name in [
            "ProjectArchiveRequest",
            "ProjectCounters",
            "ProjectListQuery",
            "ProjectListResponse",
            "ProjectRecord",
            "ProjectRegisterRequest",
        ] {
            let (_, schema) = schema_definitions()
                .into_iter()
                .find(|(schema_name, _)| *schema_name == name)
                .unwrap_or_else(|| panic!("{name} is registered"));
            let encoded = serde_json::to_string(&schema).expect("the schema serialises");
            assert!(
                encoded.contains("\"additionalProperties\":false"),
                "{name} should reject unknown fields"
            );
        }
    }

    #[test]
    fn the_list_query_is_empty_but_strict() {
        let decoded: ProjectListQuery = serde_json::from_value(json!({})).expect("it is empty");
        assert_eq!(decoded, ProjectListQuery {});

        let refused: Result<ProjectListQuery, _> = serde_json::from_value(json!({ "all": true }));
        assert!(refused.is_err(), "unknown fields are rejected");
    }
}
