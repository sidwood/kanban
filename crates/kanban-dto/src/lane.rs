//! Lane payload definitions: the durable execution slot record and
//! its assignment commands (KAN-S6-US2).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The Lane record as every client sees it: a durable execution slot
/// of one Project, holding at most one Workspace claim and at most
/// one active Ticket (DR-LW-01 to DR-LW-03).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LaneRecord {
    pub id: u64,
    pub project_id: u64,
    /// The Workspace this Lane runs in, when claimed.
    pub workspace_id: Option<u64>,
    /// The Ticket this Lane holds, when one is active.
    pub ticket_id: Option<u64>,
    pub version: u64,
}

/// Request payload for the `lane.create` command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LaneCreateRequest {
    pub mutation: super::MutationContext,
    pub project_id: u64,
}

/// Request payload for the `lane.workspace.assign` command. The Seed
/// Workspace is refused and the refusal recorded (DR-LW-07).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LaneWorkspaceAssignRequest {
    pub mutation: super::MutationContext,
    pub lane_id: u64,
    pub workspace_id: u64,
}

/// Request payload for the `lane.workspace.release` command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LaneWorkspaceReleaseRequest {
    pub mutation: super::MutationContext,
    pub lane_id: u64,
}

/// Request payload for the `lane.ticket.assign` command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LaneTicketAssignRequest {
    pub mutation: super::MutationContext,
    pub lane_id: u64,
    pub ticket_id: u64,
}

/// Request payload for the `lane.ticket.release` command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LaneTicketReleaseRequest {
    pub mutation: super::MutationContext,
    pub lane_id: u64,
}

/// Request payload for the `lane.list` query.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LaneListQuery {
    pub project_id: u64,
}

/// Response payload for the `lane.list` query.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LaneListResponse {
    pub lanes: Vec<LaneRecord>,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        LaneCreateRequest, LaneListQuery, LaneRecord, LaneTicketAssignRequest,
        LaneTicketReleaseRequest, LaneWorkspaceAssignRequest, LaneWorkspaceReleaseRequest,
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
    fn lane_create_request_rejects_unknown_fields() {
        let payload = json!({
            "mutation": context(),
            "project_id": 1,
            "name": "express",
        });

        let error = serde_json::from_value::<LaneCreateRequest>(payload)
            .expect_err("unknown fields are rejected");

        assert!(error.to_string().contains("unknown field"));
    }

    #[test]
    fn lane_workspace_assign_request_rejects_unknown_fields() {
        let payload = json!({
            "mutation": context(),
            "lane_id": 1,
            "workspace_id": 2,
            "force": true,
        });

        let error = serde_json::from_value::<LaneWorkspaceAssignRequest>(payload)
            .expect_err("unknown fields are rejected");

        assert!(error.to_string().contains("unknown field"));
    }

    #[test]
    fn lane_workspace_release_request_rejects_unknown_fields() {
        let payload = json!({
            "mutation": context(),
            "lane_id": 1,
            "workspace_id": 2,
        });

        let error = serde_json::from_value::<LaneWorkspaceReleaseRequest>(payload)
            .expect_err("unknown fields are rejected");

        assert!(error.to_string().contains("unknown field"));
    }

    #[test]
    fn lane_ticket_assign_request_rejects_unknown_fields() {
        let payload = json!({
            "mutation": context(),
            "lane_id": 1,
            "ticket_id": 5,
            "queued": true,
        });

        let error = serde_json::from_value::<LaneTicketAssignRequest>(payload)
            .expect_err("unknown fields are rejected");

        assert!(error.to_string().contains("unknown field"));
    }

    #[test]
    fn lane_ticket_release_request_rejects_unknown_fields() {
        let payload = json!({
            "mutation": context(),
            "lane_id": 1,
            "ticket_id": 5,
        });

        let error = serde_json::from_value::<LaneTicketReleaseRequest>(payload)
            .expect_err("unknown fields are rejected");

        assert!(error.to_string().contains("unknown field"));
    }

    #[test]
    fn lane_list_query_rejects_unknown_fields() {
        let payload = json!({ "project_id": 1, "include_empty": true });

        let error = serde_json::from_value::<LaneListQuery>(payload)
            .expect_err("unknown fields are rejected");

        assert!(error.to_string().contains("unknown field"));
    }

    #[test]
    fn lane_record_round_trips_its_claims() {
        let record = LaneRecord {
            id: 1,
            project_id: 2,
            workspace_id: Some(3),
            ticket_id: Some(5),
            version: 4,
        };

        let encoded = serde_json::to_value(&record).expect("the record encodes");
        assert_eq!(encoded["workspace_id"], json!(3));
        assert_eq!(encoded["ticket_id"], json!(5));
        let decoded: LaneRecord = serde_json::from_value(encoded).expect("the record decodes");
        assert_eq!(decoded, record);
    }

    #[test]
    fn lane_record_round_trips_an_empty_slot() {
        let record = LaneRecord {
            id: 1,
            project_id: 2,
            workspace_id: None,
            ticket_id: None,
            version: 1,
        };

        let encoded = serde_json::to_value(&record).expect("the record encodes");
        assert_eq!(encoded["workspace_id"], json!(null));
        assert_eq!(encoded["ticket_id"], json!(null));
        let decoded: LaneRecord = serde_json::from_value(encoded).expect("the record decodes");
        assert_eq!(decoded, record);
    }

    #[test]
    fn lane_payloads_are_in_the_schema_registry() {
        let names: Vec<_> = schema_definitions()
            .into_iter()
            .map(|(name, _)| name)
            .collect();

        for expected in [
            "LaneCreateRequest",
            "LaneListQuery",
            "LaneListResponse",
            "LaneRecord",
            "LaneTicketAssignRequest",
            "LaneTicketReleaseRequest",
            "LaneWorkspaceAssignRequest",
            "LaneWorkspaceReleaseRequest",
        ] {
            assert!(names.contains(&expected), "{expected} must be registered");
        }
    }
}
