//! Guarded branch-clone payload definitions (KAN-S6-US4). The fleet's
//! `git bc-add` family is the only sanctioned clone mechanism; these
//! payloads carry the guarded create and remove commands and the
//! records they answer with.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Request payload for the `clone.create` command. The source is not
/// client-supplied: Kanban resolves the Project's registered
/// repository and refuses conflicting paths, branches, and Lane
/// assignments before invoking anything (DR-LW-09, DR-LW-10).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CloneCreateRequest {
    pub mutation: super::MutationContext,
    pub project_id: u64,
    /// The directory the branch clone lands in.
    pub path: String,
    /// The branch the clone checks out.
    pub branch: String,
}

/// Request payload for the `clone.remove` command. The Workspace
/// record is preserved, never deleted or retired (DR-LW-11); only the
/// clone on disk goes, and only through the guarded fleet skill. The
/// removal itself marks the Workspace missing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CloneRemoveRequest {
    pub mutation: super::MutationContext,
    pub workspace_id: u64,
}

/// The record a guarded create answers with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CloneCreatedRecord {
    pub project_id: u64,
    /// The directory the branch clone landed in.
    pub path: String,
    /// The branch the clone checked out.
    pub branch: String,
}

/// The record a guarded remove answers with. The Workspace record
/// itself is preserved and the removal marks it missing, so the
/// branch named here is the one the Workspace last observed — the
/// checkout that answered is already gone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CloneRemovedRecord {
    pub project_id: u64,
    pub workspace_id: u64,
    /// The directory the branch clone was removed from.
    pub path: String,
    /// The branch the Workspace last observed, when any.
    pub branch: Option<String>,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{CloneCreateRequest, CloneCreatedRecord, CloneRemoveRequest, CloneRemovedRecord};
    use crate::mutation::MutationContext;
    use crate::schema_definitions;

    fn context() -> MutationContext {
        MutationContext {
            optimistic_version: 0,
            idempotency_key: "key-1".to_owned(),
        }
    }

    #[test]
    fn clone_create_request_rejects_unknown_fields() {
        let payload = json!({
            "mutation": context(),
            "project_id": 1,
            "path": "/workspaces/kanban.fleet-t34",
            "branch": "fleet/kan-t34",
            "source": "/workspaces/kanban.seed",
        });

        let error = serde_json::from_value::<CloneCreateRequest>(payload)
            .expect_err("unknown fields are rejected");

        assert!(error.to_string().contains("unknown field"));
    }

    #[test]
    fn clone_remove_request_rejects_unknown_fields() {
        let payload = json!({
            "mutation": context(),
            "workspace_id": 2,
            "force": true,
        });

        let error = serde_json::from_value::<CloneRemoveRequest>(payload)
            .expect_err("unknown fields are rejected");

        assert!(error.to_string().contains("unknown field"));
    }

    #[test]
    fn clone_records_round_trip() {
        let created = CloneCreatedRecord {
            project_id: 1,
            path: "/workspaces/kanban.fleet-t34".to_owned(),
            branch: "fleet/kan-t34".to_owned(),
        };
        let encoded = serde_json::to_value(&created).expect("the record encodes");
        let decoded: CloneCreatedRecord =
            serde_json::from_value(encoded).expect("the record decodes");
        assert_eq!(decoded, created);

        let removed = CloneRemovedRecord {
            project_id: 1,
            workspace_id: 2,
            path: "/workspaces/kanban.fleet-t34".to_owned(),
            branch: Some("fleet/kan-t34".to_owned()),
        };
        let encoded = serde_json::to_value(&removed).expect("the record encodes");
        let decoded: CloneRemovedRecord =
            serde_json::from_value(encoded).expect("the record decodes");
        assert_eq!(decoded, removed);
    }

    #[test]
    fn a_removed_clone_without_an_observed_branch_round_trips() {
        let removed = CloneRemovedRecord {
            project_id: 1,
            workspace_id: 2,
            path: "/workspaces/kanban.blind".to_owned(),
            branch: None,
        };

        let encoded = serde_json::to_value(&removed).expect("the record encodes");

        assert_eq!(encoded["branch"], json!(null));
        let decoded: CloneRemovedRecord =
            serde_json::from_value(encoded).expect("the record decodes");
        assert_eq!(decoded, removed);
    }

    #[test]
    fn clone_payloads_are_in_the_schema_registry() {
        let names: Vec<_> = schema_definitions()
            .into_iter()
            .map(|(name, _)| name)
            .collect();

        for expected in [
            "CloneCreateRequest",
            "CloneCreatedRecord",
            "CloneRemoveRequest",
            "CloneRemovedRecord",
        ] {
            assert!(names.contains(&expected), "{expected} must be registered");
        }
    }
}
