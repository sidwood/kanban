use schemars::schema::RootSchema;
use schemars::schema_for;

use crate::comment::{
    CommentCreateRequest, CommentEditRequest, CommentRecord, CommentRevisionRecord,
    CommentRevisionsQuery, CommentRevisionsResponse,
};
use crate::deferral::{
    DeferralListQuery, DeferralListResponse, DeferralRecord, DeferralRecordRequest,
    DeferralSupersedeRequest,
};
use crate::error::{ApiError, ErrorCode};
use crate::event::EventEnvelope;
use crate::health::{HealthQuery, HealthResponse};
use crate::initiative::{
    InitiativeArchiveRequest, InitiativeCreateRequest, InitiativeListQuery, InitiativeListResponse,
    InitiativeRecord, InitiativeRenameRequest,
};
use crate::mutation::MutationContext;
use crate::ruling::{
    RulingListQuery, RulingListResponse, RulingRecord, RulingRecordRequest, RulingSupersedeRequest,
};
use crate::timeline::{
    TimelineEntityKind, TimelineEntityRef, TimelineEventKind, TimelineEventRecord, TimelineQuery,
    TimelineQueryResponse, TimelineScope,
};

/// Every DTO schema exported to `packages/contracts`.
pub fn schema_definitions() -> Vec<(&'static str, RootSchema)> {
    vec![
        ("ApiError", schema_for!(ApiError)),
        ("CommentCreateRequest", schema_for!(CommentCreateRequest)),
        ("CommentEditRequest", schema_for!(CommentEditRequest)),
        ("CommentRecord", schema_for!(CommentRecord)),
        ("CommentRevisionRecord", schema_for!(CommentRevisionRecord)),
        ("CommentRevisionsQuery", schema_for!(CommentRevisionsQuery)),
        (
            "CommentRevisionsResponse",
            schema_for!(CommentRevisionsResponse),
        ),
        ("ErrorCode", schema_for!(ErrorCode)),
        ("EventEnvelope", schema_for!(EventEnvelope)),
        ("HealthQuery", schema_for!(HealthQuery)),
        ("HealthResponse", schema_for!(HealthResponse)),
        (
            "InitiativeArchiveRequest",
            schema_for!(InitiativeArchiveRequest),
        ),
        (
            "InitiativeCreateRequest",
            schema_for!(InitiativeCreateRequest),
        ),
        ("InitiativeListQuery", schema_for!(InitiativeListQuery)),
        (
            "InitiativeListResponse",
            schema_for!(InitiativeListResponse),
        ),
        ("InitiativeRecord", schema_for!(InitiativeRecord)),
        (
            "InitiativeRenameRequest",
            schema_for!(InitiativeRenameRequest),
        ),
        ("MutationContext", schema_for!(MutationContext)),
        ("DeferralListQuery", schema_for!(DeferralListQuery)),
        ("DeferralListResponse", schema_for!(DeferralListResponse)),
        ("DeferralRecord", schema_for!(DeferralRecord)),
        ("DeferralRecordRequest", schema_for!(DeferralRecordRequest)),
        (
            "DeferralSupersedeRequest",
            schema_for!(DeferralSupersedeRequest),
        ),
        ("RulingListQuery", schema_for!(RulingListQuery)),
        ("RulingListResponse", schema_for!(RulingListResponse)),
        ("RulingRecord", schema_for!(RulingRecord)),
        ("RulingRecordRequest", schema_for!(RulingRecordRequest)),
        (
            "RulingSupersedeRequest",
            schema_for!(RulingSupersedeRequest),
        ),
        ("TimelineEntityKind", schema_for!(TimelineEntityKind)),
        ("TimelineEntityRef", schema_for!(TimelineEntityRef)),
        ("TimelineEventKind", schema_for!(TimelineEventKind)),
        ("TimelineEventRecord", schema_for!(TimelineEventRecord)),
        ("TimelineQuery", schema_for!(TimelineQuery)),
        ("TimelineQueryResponse", schema_for!(TimelineQueryResponse)),
        ("TimelineScope", schema_for!(TimelineScope)),
    ]
}
