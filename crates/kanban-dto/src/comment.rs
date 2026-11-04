//! Comment payload definitions: the record every client sees, the
//! create and edit commands, and the revision history query
//! (KAN-S2-US2).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::timeline::TimelineEntityRef;

/// One immutable revision as returned by queries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CommentRevisionRecord {
    /// The one-based revision number.
    pub revision: u64,
    /// The revision text.
    pub text: String,
    /// When the revision was recorded.
    pub recorded_at: String,
}

/// The Comment record as every client sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CommentRecord {
    /// The immutable, storage-assigned identity.
    pub id: u64,
    /// The owning Project.
    pub project_id: u64,
    /// The entity this Comment attaches to.
    pub target: TimelineEntityRef,
    /// The current text, resolved from the latest revision.
    pub text: String,
    /// The aggregate version, for optimistic mutation checks.
    pub version: u64,
}

/// Request payload for the `comment.create` command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CommentCreateRequest {
    pub mutation: super::MutationContext,
    /// The owning Project, resolved before any write.
    pub project_id: u64,
    /// The entity this Comment attaches to.
    pub target: TimelineEntityRef,
    /// The initial text; blank text is refused.
    pub text: String,
    /// The author the comment speaks for; required, because the core
    /// holds no operator identity to default from.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub author: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_comment_id: Option<u64>,
}

/// Request payload for the `comment.edit` command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CommentEditRequest {
    pub mutation: super::MutationContext,
    /// The Comment being edited.
    pub comment_id: u64,
    /// The revised text; blank text is refused.
    pub text: String,
}

/// Request payload for the `comment.resolve` command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CommentResolveRequest {
    pub mutation: super::MutationContext,
    pub comment_id: u64,
    pub resolved: bool,
}

/// Response payload for the `comment.resolve` command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CommentResolveResponse {
    /// The Comment whose resolution changed.
    pub comment_id: u64,
    /// Whether the Comment is now resolved.
    pub resolved: bool,
    /// The aggregate version after the change, for optimistic checks.
    pub version: u64,
}

/// Request payload for the `comment.revisions` query.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CommentRevisionsQuery {
    /// The Comment whose history is requested.
    pub comment_id: u64,
}

/// Response payload for the `comment.revisions` query.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CommentRevisionsResponse {
    /// The Comment with its current text resolved.
    pub comment: CommentRecord,
    /// Every revision, oldest first.
    pub revisions: Vec<CommentRevisionRecord>,
}

/// Request payload for the `comment.list` query.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CommentListQuery {
    /// The entity whose comments are listed.
    pub target: TimelineEntityRef,
    #[serde(default)]
    pub limit: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}

/// One comment row for list views (operator MCP).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CommentListItem {
    pub id: u64,
    pub author: String,
    pub body: String,
    pub created_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edited_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_comment_id: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_at: Option<String>,
    pub version: u64,
}

/// Response payload for the `comment.list` query.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CommentListResponse {
    pub comments: Vec<CommentListItem>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        CommentCreateRequest, CommentEditRequest, CommentListItem, CommentListQuery,
        CommentListResponse, CommentRecord, CommentResolveResponse, CommentRevisionRecord,
        CommentRevisionsQuery, CommentRevisionsResponse,
    };
    use crate::mutation::MutationContext;
    use crate::schema_definitions;
    use crate::timeline::{TimelineEntityKind, TimelineEntityRef};

    fn context() -> MutationContext {
        MutationContext {
            optimistic_version: 0,
            idempotency_key: "key-1".to_owned(),
        }
    }

    fn target() -> TimelineEntityRef {
        TimelineEntityRef {
            kind: TimelineEntityKind::Ticket,
            id: "kan-t11".to_owned(),
        }
    }

    #[test]
    fn a_record_round_trips() {
        let record = CommentRecord {
            id: 3,
            project_id: 1,
            target: target(),
            text: "Ship it".to_owned(),
            version: 2,
        };

        let encoded = serde_json::to_value(&record).expect("the record serialises");
        assert_eq!(
            encoded,
            json!({
                "id": 3,
                "project_id": 1,
                "target": { "kind": "ticket", "id": "kan-t11" },
                "text": "Ship it",
                "version": 2,
            })
        );
        let decoded: CommentRecord =
            serde_json::from_value(encoded).expect("the record deserialises");
        assert_eq!(decoded, record);
    }

    #[test]
    fn create_and_edit_requests_reject_unknown_fields() {
        let create = CommentCreateRequest {
            mutation: context(),
            project_id: 1,
            target: target(),
            text: "First".to_owned(),
            author: "tester".to_owned(),
            parent_comment_id: None,
        };
        let encoded = serde_json::to_value(&create).expect("the request serialises");
        let decoded: CommentCreateRequest =
            serde_json::from_value(encoded).expect("the request deserialises");
        assert_eq!(decoded, create);

        let refused: Result<CommentCreateRequest, _> = serde_json::from_value(json!({
            "mutation": context(),
            "project_id": 1,
            "target": target(),
            "text": "First",
            "surprise": true,
        }));
        assert!(refused.is_err(), "unknown fields are rejected");

        let edit = CommentEditRequest {
            mutation: context(),
            comment_id: 1,
            text: "Second".to_owned(),
        };
        let encoded = serde_json::to_value(&edit).expect("the request serialises");
        let decoded: CommentEditRequest =
            serde_json::from_value(encoded).expect("the request deserialises");
        assert_eq!(decoded, edit);
    }

    #[test]
    fn a_resolve_response_carries_the_bumped_version() {
        let response = CommentResolveResponse {
            comment_id: 4,
            resolved: true,
            version: 2,
        };
        let encoded = serde_json::to_value(&response).expect("the response serialises");
        assert_eq!(
            encoded,
            json!({ "comment_id": 4, "resolved": true, "version": 2 })
        );
        let decoded: CommentResolveResponse =
            serde_json::from_value(encoded).expect("the response deserialises");
        assert_eq!(decoded, response);
    }

    #[test]
    fn the_create_request_advertises_its_author_as_required() {
        let (_, schema) = schema_definitions()
            .into_iter()
            .find(|(schema_name, _)| *schema_name == "CommentCreateRequest")
            .expect("CommentCreateRequest is registered");

        let required: Vec<_> = schema
            .schema
            .object
            .as_ref()
            .expect("the request schema is an object")
            .required
            .iter()
            .collect();
        assert!(
            required.contains(&&"author".to_owned()),
            "the contract must demand the author the runtime requires: {required:?}"
        );

        let encoded = serde_json::to_value(&schema).expect("the schema serialises");
        let author = &encoded["properties"]["author"];
        assert_eq!(
            author["type"],
            json!("string"),
            "the author stays a plain string"
        );
        let description = author["description"].as_str().expect("described");
        assert!(
            description.contains("required"),
            "the description must not promise an optional author: {description}"
        );
    }

    #[test]
    fn a_revisions_response_carries_history() {
        let response = CommentRevisionsResponse {
            comment: CommentRecord {
                id: 1,
                project_id: 1,
                target: target(),
                text: "Latest".to_owned(),
                version: 2,
            },
            revisions: vec![
                CommentRevisionRecord {
                    revision: 1,
                    text: "First".to_owned(),
                    recorded_at: "2026-09-04T12:00:01Z".to_owned(),
                },
                CommentRevisionRecord {
                    revision: 2,
                    text: "Latest".to_owned(),
                    recorded_at: "2026-09-04T12:00:02Z".to_owned(),
                },
            ],
        };

        let encoded = serde_json::to_value(&response).expect("the response serialises");
        assert_eq!(encoded["revisions"].as_array().map(Vec::len), Some(2));
        let decoded: CommentRevisionsResponse =
            serde_json::from_value(encoded).expect("the response deserialises");
        assert_eq!(decoded, response);
    }

    #[test]
    fn a_list_response_carries_comments_and_cursor() {
        let query = CommentListQuery {
            target: target(),
            limit: 50,
            cursor: None,
        };
        let encoded_query = serde_json::to_value(&query).expect("the query serialises");
        let decoded_query: CommentListQuery =
            serde_json::from_value(encoded_query).expect("the query deserialises");
        assert_eq!(decoded_query, query);

        let response = CommentListResponse {
            comments: vec![CommentListItem {
                id: 1,
                author: "ada".to_owned(),
                body: "Ship report".to_owned(),
                created_at: "2026-09-04T12:00:01Z".to_owned(),
                edited_at: None,
                parent_comment_id: None,
                resolved_at: None,
                version: 1,
            }],
            next_cursor: Some("1".to_owned()),
        };
        let encoded = serde_json::to_value(&response).expect("the response serialises");
        assert_eq!(encoded["comments"].as_array().map(Vec::len), Some(1));
        let decoded: CommentListResponse =
            serde_json::from_value(encoded).expect("the response deserialises");
        assert_eq!(decoded, response);
    }

    #[test]
    fn every_comment_schema_rejects_unknown_fields() {
        for name in [
            "CommentCreateRequest",
            "CommentEditRequest",
            "CommentRecord",
            "CommentRevisionRecord",
            "CommentResolveResponse",
            "CommentRevisionsQuery",
            "CommentRevisionsResponse",
            "CommentListQuery",
            "CommentListResponse",
            "CommentListItem",
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
    fn the_revisions_query_is_strict() {
        let decoded: CommentRevisionsQuery =
            serde_json::from_value(json!({ "comment_id": 1 })).expect("it decodes");
        assert_eq!(decoded.comment_id, 1);

        let refused: Result<CommentRevisionsQuery, _> =
            serde_json::from_value(json!({ "comment_id": 1, "all": true }));
        assert!(refused.is_err(), "unknown fields are rejected");
    }
}
