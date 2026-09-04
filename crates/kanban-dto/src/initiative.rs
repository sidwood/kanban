//! Initiative payload definitions: the record every client sees and
//! the create, rename, archive, and list payloads (KAN-S1-US3,
//! KAN-S1-US6). There is deliberately no delete payload.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The Initiative record as every client sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InitiativeRecord {
    /// The immutable, storage-assigned identity.
    pub id: u64,
    /// The trimmed name.
    pub name: String,
    /// Whether the Initiative is archived. Archived Initiatives stay
    /// listed; no fact is ever removed.
    pub archived: bool,
    /// The aggregate version, for optimistic mutation checks.
    pub version: u64,
}

/// Request payload for the `initiative.create` command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InitiativeCreateRequest {
    pub mutation: super::MutationContext,
    /// The intended name; blank names are refused.
    pub name: String,
}

/// Request payload for the `initiative.rename` command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InitiativeRenameRequest {
    pub mutation: super::MutationContext,
    /// The Initiative being renamed.
    pub initiative_id: u64,
    /// The intended name; blank names are refused.
    pub name: String,
}

/// Request payload for the `initiative.archive` command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InitiativeArchiveRequest {
    pub mutation: super::MutationContext,
    /// The Initiative being archived.
    pub initiative_id: u64,
}

/// Request payload for the `initiative.list` query.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InitiativeListQuery {}

/// Response payload for the `initiative.list` query.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InitiativeListResponse {
    /// Every Initiative, archived included, newest last.
    pub initiatives: Vec<InitiativeRecord>,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        InitiativeArchiveRequest, InitiativeCreateRequest, InitiativeListQuery,
        InitiativeListResponse, InitiativeRecord, InitiativeRenameRequest,
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
    fn a_record_round_trips() {
        let record = InitiativeRecord {
            id: 3,
            name: "Reliability".to_owned(),
            archived: false,
            version: 2,
        };

        let encoded = serde_json::to_value(&record).expect("the record serialises");
        assert_eq!(
            encoded,
            json!({
                "id": 3,
                "name": "Reliability",
                "archived": false,
                "version": 2,
            })
        );
        let decoded: InitiativeRecord =
            serde_json::from_value(encoded).expect("the record deserialises");
        assert_eq!(decoded, record);
    }

    #[test]
    fn a_create_request_round_trips_and_rejects_unknown_fields() {
        let request = InitiativeCreateRequest {
            mutation: context(),
            name: "Alpha".to_owned(),
        };

        let encoded = serde_json::to_value(&request).expect("the request serialises");
        let decoded: InitiativeCreateRequest =
            serde_json::from_value(encoded).expect("the request deserialises");
        assert_eq!(decoded, request);

        let refused: Result<InitiativeCreateRequest, _> = serde_json::from_value(
            json!({ "mutation": context(), "name": "Alpha", "delete": true }),
        );
        assert!(refused.is_err(), "unknown fields are rejected");
    }

    #[test]
    fn a_rename_request_names_its_initiative() {
        let request = InitiativeRenameRequest {
            mutation: context(),
            initiative_id: 5,
            name: "Beta".to_owned(),
        };

        let encoded = serde_json::to_value(&request).expect("the request serialises");
        assert_eq!(encoded["initiative_id"], json!(5));
        let decoded: InitiativeRenameRequest =
            serde_json::from_value(encoded).expect("the request deserialises");
        assert_eq!(decoded, request);
    }

    #[test]
    fn an_archive_request_holds_the_identity_and_context_only() {
        let request = InitiativeArchiveRequest {
            mutation: context(),
            initiative_id: 5,
        };

        let encoded = serde_json::to_value(&request).expect("the request serialises");
        assert_eq!(
            encoded,
            json!({ "mutation": { "optimistic_version": 0, "idempotency_key": "key-1" }, "initiative_id": 5 })
        );
    }

    #[test]
    fn a_list_response_carries_its_records() {
        let response = InitiativeListResponse {
            initiatives: vec![
                InitiativeRecord {
                    id: 1,
                    name: "Alpha".to_owned(),
                    archived: true,
                    version: 2,
                },
                InitiativeRecord {
                    id: 2,
                    name: "Beta".to_owned(),
                    archived: false,
                    version: 1,
                },
            ],
        };

        let encoded = serde_json::to_value(&response).expect("the response serialises");
        assert_eq!(encoded["initiatives"].as_array().map(Vec::len), Some(2));
        let decoded: InitiativeListResponse =
            serde_json::from_value(encoded).expect("the response deserialises");
        assert_eq!(decoded, response);
    }

    #[test]
    fn every_initiative_schema_rejects_unknown_fields() {
        for name in [
            "InitiativeArchiveRequest",
            "InitiativeCreateRequest",
            "InitiativeListQuery",
            "InitiativeListResponse",
            "InitiativeRecord",
            "InitiativeRenameRequest",
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
        let decoded: InitiativeListQuery = serde_json::from_value(json!({})).expect("it is empty");
        assert_eq!(decoded, InitiativeListQuery {});

        let refused: Result<InitiativeListQuery, _> =
            serde_json::from_value(json!({ "all": true }));
        assert!(refused.is_err(), "unknown fields are rejected");
    }
}
