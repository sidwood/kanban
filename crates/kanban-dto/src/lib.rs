//! Authoritative command, query, event, and error payload definitions
//! with schema derivation. Depends on nothing internal.

pub mod comment;
pub mod coverage;
pub mod deferral;
pub mod diagnostics;
pub mod error;
pub mod event;
pub mod evidence;
pub mod export;
pub mod health;
pub mod herdr;
pub mod initiative;
pub mod lane;
pub mod mutation;
pub mod plan;
pub mod profile;
pub mod project;
pub mod ruling;
pub mod schema;
pub mod spec;
pub mod ticket;
pub mod timeline;

pub mod workspace;

pub use comment::{
    CommentCreateRequest, CommentEditRequest, CommentRecord, CommentRevisionRecord,
    CommentRevisionsQuery, CommentRevisionsResponse,
};
pub use coverage::{
    CoverageCriterionProposal, CriterionRefusal, RefusedCriterion, SpecCoverageCheckQuery,
    SpecCoverageCheckResponse,
};
pub use deferral::{
    DeferralListQuery, DeferralListResponse, DeferralRecord, DeferralRecordRequest,
    DeferralSupersedeRequest,
};
pub use diagnostics::{DiagnosticsExportQuery, DiagnosticsExportResponse};
pub use error::{ApiError, ErrorCode};
pub use event::{
    DecodeLiveEventError, DeferralIdentity, EventEnvelope, EvidenceListSummary,
    InvalidLiveEventPayloadError, LiveEvent, LiveEventDescriptor, LiveEventName, RulingIdentity,
    UnknownLiveEventError, decode_live_event, event_descriptor, live_event_catalog,
};
pub use evidence::{
    EvidenceAttachRequest, EvidenceKindDto, EvidenceListQuery, EvidenceListResponse, EvidenceRecord,
};
pub use export::{
    ExportDriftEntry, ExportDriftQuery, ExportDriftResponse, ExportDriftStatus,
    ExportRenderRequest, ExportRenderResponse,
};
pub use health::{HealthQuery, HealthResponse};
pub use herdr::{
    HerdrConnectionDiagnostics, HerdrDefaultsGetQuery, HerdrDefaultsGetResponse,
    HerdrDefaultsUpdateRequest, HerdrGlobalDefaults, HerdrProjectSettings, HerdrSettingsGetQuery,
    HerdrSettingsGetResponse, HerdrSettingsUpdateRequest,
};
pub use initiative::{
    InitiativeArchiveRequest, InitiativeCreateRequest, InitiativeListQuery, InitiativeListResponse,
    InitiativeRecord, InitiativeRenameRequest,
};
pub use lane::{
    LaneCreateRequest, LaneListQuery, LaneListResponse, LaneRecord, LaneTicketAssignRequest,
    LaneTicketReleaseRequest, LaneWorkspaceAssignRequest, LaneWorkspaceReleaseRequest,
};
pub use mutation::MutationContext;
pub use plan::{
    PlanActivateRequest, PlanArchiveRequest, PlanCancelRequest, PlanCompleteRequest,
    PlanCoverageGap, PlanCreateRequest, PlanCycle, PlanDiagnosticsQuery, PlanDiagnosticsResponse,
    PlanEdge, PlanEdgeAddRequest, PlanEdgeRemoveRequest, PlanGetQuery, PlanGetResponse,
    PlanInvalidProfile, PlanListQuery, PlanListResponse, PlanRecord, PlanReplanRequest,
    PlanSpecAddRequest, PlanSpecMoveRequest, PlanSpecRemoveRequest, PlanState, PlanVersionRecord,
};
pub use profile::{
    ProfileDefineRequest, ProfileGetQuery, ProfileListQuery, ProfileListResponse, ProfileRecord,
    ProfileRetireRequest, ProfileUpdateRequest,
};
pub use project::{
    ProjectArchiveRequest, ProjectCounters, ProjectListQuery, ProjectListResponse, ProjectRecord,
    ProjectRegisterRequest,
};
pub use ruling::{
    RulingListQuery, RulingListResponse, RulingRecord, RulingRecordRequest, RulingSupersedeRequest,
};
pub use schema::schema_definitions;
pub use spec::{
    SpecContent, SpecContentState, SpecContentUpdateRequest, SpecCreateRequest,
    SpecExecutionMoveRequest, SpecExecutionState, SpecGetQuery, SpecGetResponse, SpecListQuery,
    SpecListResponse, SpecPlanJoinRequest, SpecRecord, SpecVersionApproveRequest,
    SpecVersionGetQuery, SpecVersionRecord, SpecVersionSupersedeRequest,
};
pub use ticket::{
    TaskMode, TaskSubtype, TicketAssignRequest, TicketBlockerAddRequest, TicketBlockerRecord,
    TicketBlockerRemoveRequest, TicketBugFactsRequest, TicketBugQualification,
    TicketBugQualifyRequest, TicketBugRecord, TicketCreateRequest, TicketCriterion,
    TicketDependenciesQuery, TicketDependenciesResponse, TicketDependencyAddRequest,
    TicketDependencyRecord, TicketDependencyRemoveRequest, TicketExternalReference, TicketGetQuery,
    TicketKind, TicketListQuery, TicketListResponse, TicketOccurrenceSnapshot, TicketPriority,
    TicketReadinessBlocker, TicketReadinessQuery, TicketReadinessResponse, TicketRecord,
    TicketSeverity, TicketState, TicketVerificationStep,
};
pub use timeline::{
    TimelineEntityKind, TimelineEntityRef, TimelineEventKind, TimelineEventRecord, TimelineQuery,
    TimelineQueryResponse, TimelineScope,
};
pub use workspace::{
    WorkspaceCheckoutDto, WorkspaceHealthDto, WorkspaceListQuery, WorkspaceListResponse,
    WorkspaceObservationDto, WorkspaceObserveRequest, WorkspaceRecord, WorkspaceRegisterRequest,
    WorkspaceRetireRequest, WorkspaceReuseDto,
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
                "CoverageCriterionProposal",
                "CriterionRefusal",
                "ErrorCode",
                "DeferralIdentity",
                "DiagnosticsExportQuery",
                "DiagnosticsExportResponse",
                "EventEnvelope",
                "ExportDriftEntry",
                "ExportDriftQuery",
                "ExportDriftResponse",
                "ExportDriftStatus",
                "ExportRenderRequest",
                "ExportRenderResponse",
                "EvidenceAttachRequest",
                "EvidenceKindDto",
                "EvidenceListQuery",
                "EvidenceListResponse",
                "EvidenceListSummary",
                "EvidenceRecord",
                "LiveEventName",
                "HealthQuery",
                "HealthResponse",
                "HerdrConnectionDiagnostics",
                "HerdrDefaultsGetQuery",
                "HerdrDefaultsGetResponse",
                "HerdrDefaultsUpdateRequest",
                "HerdrGlobalDefaults",
                "HerdrProjectSettings",
                "HerdrSettingsGetQuery",
                "HerdrSettingsGetResponse",
                "HerdrSettingsUpdateRequest",
                "InitiativeArchiveRequest",
                "InitiativeCreateRequest",
                "InitiativeListQuery",
                "InitiativeListResponse",
                "InitiativeRecord",
                "InitiativeRenameRequest",
                "MutationContext",
                "PlanActivateRequest",
                "PlanArchiveRequest",
                "PlanCancelRequest",
                "PlanCompleteRequest",
                "PlanCoverageGap",
                "PlanCreateRequest",
                "PlanCycle",
                "PlanDiagnosticsQuery",
                "PlanDiagnosticsResponse",
                "PlanEdge",
                "PlanEdgeAddRequest",
                "PlanEdgeRemoveRequest",
                "PlanGetQuery",
                "PlanGetResponse",
                "PlanInvalidProfile",
                "PlanListQuery",
                "PlanListResponse",
                "PlanRecord",
                "PlanReplanRequest",
                "PlanSpecAddRequest",
                "PlanSpecMoveRequest",
                "PlanSpecRemoveRequest",
                "PlanState",
                "PlanVersionRecord",
                "ProfileDefineRequest",
                "ProfileGetQuery",
                "ProfileListQuery",
                "ProfileListResponse",
                "ProfileRecord",
                "ProfileRetireRequest",
                "ProfileUpdateRequest",
                "SpecContent",
                "SpecContentState",
                "SpecContentUpdateRequest",
                "SpecCreateRequest",
                "SpecExecutionMoveRequest",
                "SpecExecutionState",
                "SpecGetQuery",
                "SpecGetResponse",
                "SpecListQuery",
                "SpecListResponse",
                "SpecPlanJoinRequest",
                "SpecRecord",
                "SpecVersionApproveRequest",
                "SpecVersionGetQuery",
                "SpecVersionRecord",
                "SpecVersionSupersedeRequest",
                "TicketBugFactsRequest",
                "TicketBugQualification",
                "TicketBugQualifyRequest",
                "TicketBugRecord",
                "TaskMode",
                "TaskSubtype",
                "TicketAssignRequest",
                "TicketCreateRequest",
                "TicketCriterion",
                "TicketDependencyAddRequest",
                "TicketDependencyRecord",
                "TicketDependencyRemoveRequest",
                "TicketDependenciesQuery",
                "TicketDependenciesResponse",
                "TicketExternalReference",
                "TicketGetQuery",
                "TicketKind",
                "TicketListQuery",
                "TicketListResponse",
                "TicketOccurrenceSnapshot",
                "TicketPriority",
                "TicketReadinessBlocker",
                "TicketReadinessQuery",
                "TicketReadinessResponse",
                "TicketRecord",
                "TicketSeverity",
                "TicketState",
                "TicketBlockerAddRequest",
                "TicketBlockerRecord",
                "TicketBlockerRemoveRequest",
                "TicketVerificationStep",
                "ProjectArchiveRequest",
                "ProjectCounters",
                "ProjectListQuery",
                "ProjectListResponse",
                "ProjectRecord",
                "ProjectRegisterRequest",
                "DeferralListQuery",
                "DeferralListResponse",
                "DeferralRecord",
                "DeferralRecordRequest",
                "DeferralSupersedeRequest",
                "RulingIdentity",
                "RefusedCriterion",
                "SpecCoverageCheckQuery",
                "SpecCoverageCheckResponse",
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
                "WorkspaceCheckoutDto",
                "WorkspaceHealthDto",
                "WorkspaceListQuery",
                "WorkspaceListResponse",
                "WorkspaceObserveRequest",
                "WorkspaceObservationDto",
                "WorkspaceRecord",
                "WorkspaceRegisterRequest",
                "WorkspaceRetireRequest",
                "WorkspaceReuseDto",
                "LaneRecord",
                "LaneCreateRequest",
                "LaneWorkspaceAssignRequest",
                "LaneWorkspaceReleaseRequest",
                "LaneTicketAssignRequest",
                "LaneTicketReleaseRequest",
                "LaneListQuery",
                "LaneListResponse",
            ]
        );
    }
}
