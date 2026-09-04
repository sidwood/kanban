//! Authoritative command, query, event, and error payload definitions
//! with schema derivation. Depends on nothing internal.

pub mod comment;
pub mod deferral;
pub mod error;
pub mod event;
pub mod evidence;
pub mod health;
pub mod initiative;
pub mod mutation;
pub mod ruling;
pub mod schema;
pub mod timeline;

pub use comment::{
    CommentCreateRequest, CommentEditRequest, CommentRecord, CommentRevisionRecord,
    CommentRevisionsQuery, CommentRevisionsResponse,
};
pub use deferral::{
    DeferralListQuery, DeferralListResponse, DeferralRecord, DeferralRecordRequest,
    DeferralSupersedeRequest,
};
pub use error::{ApiError, ErrorCode};
pub use event::EventEnvelope;
pub use evidence::{
    EvidenceAttachRequest, EvidenceKindDto, EvidenceListRequest, EvidenceListResponse,
    EvidenceRecord,
};
pub use health::{HealthQuery, HealthResponse};
pub use initiative::{
    InitiativeArchiveRequest, InitiativeCreateRequest, InitiativeListQuery, InitiativeListResponse,
    InitiativeRecord, InitiativeRenameRequest,
};
pub use mutation::MutationContext;
pub use ruling::{
    RulingListQuery, RulingListResponse, RulingRecord, RulingRecordRequest, RulingSupersedeRequest,
};
pub use schema::schema_definitions;
pub use timeline::{
    TimelineEntityKind, TimelineEntityRef, TimelineEventKind, TimelineEventRecord, TimelineQuery,
    TimelineQueryResponse, TimelineScope,
};

#[cfg(test)]
mod tests {
    use schemars::schema_for;

    use super::health::{HealthQuery, HealthResponse};
    use super::schema_definitions;

    #[test]
    fn health_query_derives_json_schema() {
        let schema = schema_for!(HealthQuery);
        let json = serde_json::to_value(schema).expect("schema serialises");

        assert_eq!(
            json.get("title").and_then(|title| title.as_str()),
            Some("HealthQuery")
        );
        let encoded = serde_json::to_string(&json).expect("schema encodes");
        assert!(
            encoded.contains("\"additionalProperties\":false"),
            "HealthQuery should reject unknown fields"
        );
    }

    #[test]
    fn health_response_schema_includes_service_version() {
        let schema = schema_for!(HealthResponse);
        let json = serde_json::to_value(schema).expect("schema serialises");
        let properties = json
            .get("properties")
            .and_then(|value| value.as_object())
            .expect("HealthResponse exposes properties");

        assert!(properties.contains_key("service_version"));
        assert!(properties.contains_key("connected"));
    }

    #[test]
    fn schema_registry_lists_every_exported_dto() {
        let names: Vec<_> = schema_definitions()
            .into_iter()
            .map(|(name, _)| name)
            .collect();

        assert_eq!(
            names,
            vec![
                "ApiError",
                "CommentCreateRequest",
                "CommentEditRequest",
                "CommentRecord",
                "CommentRevisionRecord",
                "CommentRevisionsQuery",
                "CommentRevisionsResponse",
                "ErrorCode",
                "EventEnvelope",
                "EvidenceAttachRequest",
                "EvidenceKindDto",
                "EvidenceListRequest",
                "EvidenceListResponse",
                "EvidenceRecord",
                "HealthQuery",
                "HealthResponse",
                "InitiativeArchiveRequest",
                "InitiativeCreateRequest",
                "InitiativeListQuery",
                "InitiativeListResponse",
                "InitiativeRecord",
                "InitiativeRenameRequest",
                "MutationContext",
                "DeferralListQuery",
                "DeferralListResponse",
                "DeferralRecord",
                "DeferralRecordRequest",
                "DeferralSupersedeRequest",
                "RulingListQuery",
                "RulingListResponse",
                "RulingRecord",
                "RulingRecordRequest",
                "RulingSupersedeRequest",
                "TimelineEntityKind",
                "TimelineEntityRef",
                "TimelineEventKind",
                "TimelineEventRecord",
                "TimelineQuery",
                "TimelineQueryResponse",
                "TimelineScope",
            ]
        );
    }
}
