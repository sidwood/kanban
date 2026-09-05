use schemars::schema::RootSchema;
use schemars::schema_for;

use crate::comment::{
    CommentCreateRequest, CommentEditRequest, CommentRecord, CommentRevisionRecord,
    CommentRevisionsQuery, CommentRevisionsResponse,
};
use crate::coverage::{
    CoverageCriterionProposal, CriterionRefusal, RefusedCriterion, SpecCoverageCheckQuery,
    SpecCoverageCheckResponse,
};
use crate::deferral::{
    DeferralListQuery, DeferralListResponse, DeferralRecord, DeferralRecordRequest,
    DeferralSupersedeRequest,
};
use crate::diagnostics::{DiagnosticsExportQuery, DiagnosticsExportResponse};
use crate::error::{ApiError, ErrorCode};
use crate::event::{
    DeferralIdentity, EventEnvelope, EvidenceListSummary, LiveEventName, RulingIdentity,
};
use crate::evidence::{
    EvidenceAttachRequest, EvidenceKindDto, EvidenceListQuery, EvidenceListResponse, EvidenceRecord,
};
use crate::health::{HealthQuery, HealthResponse};
use crate::herdr::{
    HerdrConnectionDiagnostics, HerdrDefaultsGetQuery, HerdrDefaultsGetResponse,
    HerdrDefaultsUpdateRequest, HerdrGlobalDefaults, HerdrProjectSettings, HerdrSettingsGetQuery,
    HerdrSettingsGetResponse, HerdrSettingsUpdateRequest,
};
use crate::initiative::{
    InitiativeArchiveRequest, InitiativeCreateRequest, InitiativeListQuery, InitiativeListResponse,
    InitiativeRecord, InitiativeRenameRequest,
};
use crate::mutation::MutationContext;
use crate::plan::{
    PlanActivateRequest, PlanArchiveRequest, PlanCancelRequest, PlanCompleteRequest,
    PlanCoverageGap, PlanCreateRequest, PlanCycle, PlanDiagnosticsQuery, PlanDiagnosticsResponse,
    PlanEdge, PlanEdgeAddRequest, PlanEdgeRemoveRequest, PlanGetQuery, PlanGetResponse,
    PlanInvalidProfile, PlanListQuery, PlanListResponse, PlanRecord, PlanReplanRequest,
    PlanSpecAddRequest, PlanSpecMoveRequest, PlanSpecRemoveRequest, PlanState, PlanVersionRecord,
};
use crate::project::{
    ProjectArchiveRequest, ProjectCounters, ProjectListQuery, ProjectListResponse, ProjectRecord,
    ProjectRegisterRequest,
};
use crate::ruling::{
    RulingListQuery, RulingListResponse, RulingRecord, RulingRecordRequest, RulingSupersedeRequest,
};
use crate::spec::{
    SpecContent, SpecContentState, SpecContentUpdateRequest, SpecCreateRequest,
    SpecExecutionMoveRequest, SpecExecutionState, SpecGetQuery, SpecGetResponse, SpecListQuery,
    SpecListResponse, SpecPlanJoinRequest, SpecRecord, SpecVersionApproveRequest,
    SpecVersionGetQuery, SpecVersionRecord, SpecVersionSupersedeRequest,
};
use crate::ticket::{
    TicketBlockerAddRequest, TicketBlockerRecord, TicketBlockerRemoveRequest,
    TicketBugFactsRequest, TicketBugQualification, TicketBugQualifyRequest, TicketBugRecord,
    TicketCreateRequest, TicketCriterion, TicketDependenciesQuery, TicketDependenciesResponse,
    TicketDependencyAddRequest, TicketDependencyRecord, TicketDependencyRemoveRequest,
    TicketExternalReference, TicketGetQuery, TicketKind, TicketListQuery, TicketListResponse,
    TicketOccurrenceSnapshot, TicketPriority, TicketReadinessBlocker, TicketReadinessQuery,
    TicketReadinessResponse, TicketRecord, TicketSeverity, TicketState, TicketVerificationStep,
};
use crate::timeline::{
    TimelineEntityKind, TimelineEntityRef, TimelineEventKind, TimelineEventRecord, TimelineQuery,
    TimelineQueryResponse, TimelineScope,
};
use crate::workspace::{
    WorkspaceCheckoutDto, WorkspaceHealthDto, WorkspaceListQuery, WorkspaceListResponse,
    WorkspaceObservationDto, WorkspaceObserveRequest, WorkspaceRecord, WorkspaceRegisterRequest,
    WorkspaceRetireRequest, WorkspaceReuseDto,
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
        (
            "CoverageCriterionProposal",
            schema_for!(CoverageCriterionProposal),
        ),
        ("CriterionRefusal", schema_for!(CriterionRefusal)),
        ("ErrorCode", schema_for!(ErrorCode)),
        ("DeferralIdentity", schema_for!(DeferralIdentity)),
        (
            "DiagnosticsExportQuery",
            schema_for!(DiagnosticsExportQuery),
        ),
        (
            "DiagnosticsExportResponse",
            schema_for!(DiagnosticsExportResponse),
        ),
        ("EventEnvelope", schema_for!(EventEnvelope)),
        ("EvidenceAttachRequest", schema_for!(EvidenceAttachRequest)),
        ("EvidenceKindDto", schema_for!(EvidenceKindDto)),
        ("EvidenceListQuery", schema_for!(EvidenceListQuery)),
        ("EvidenceListResponse", schema_for!(EvidenceListResponse)),
        ("EvidenceListSummary", schema_for!(EvidenceListSummary)),
        ("EvidenceRecord", schema_for!(EvidenceRecord)),
        ("LiveEventName", schema_for!(LiveEventName)),
        ("HealthQuery", schema_for!(HealthQuery)),
        ("HealthResponse", schema_for!(HealthResponse)),
        (
            "HerdrConnectionDiagnostics",
            schema_for!(HerdrConnectionDiagnostics),
        ),
        ("HerdrDefaultsGetQuery", schema_for!(HerdrDefaultsGetQuery)),
        (
            "HerdrDefaultsGetResponse",
            schema_for!(HerdrDefaultsGetResponse),
        ),
        (
            "HerdrDefaultsUpdateRequest",
            schema_for!(HerdrDefaultsUpdateRequest),
        ),
        ("HerdrGlobalDefaults", schema_for!(HerdrGlobalDefaults)),
        ("HerdrProjectSettings", schema_for!(HerdrProjectSettings)),
        ("HerdrSettingsGetQuery", schema_for!(HerdrSettingsGetQuery)),
        (
            "HerdrSettingsGetResponse",
            schema_for!(HerdrSettingsGetResponse),
        ),
        (
            "HerdrSettingsUpdateRequest",
            schema_for!(HerdrSettingsUpdateRequest),
        ),
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
        ("PlanActivateRequest", schema_for!(PlanActivateRequest)),
        ("PlanArchiveRequest", schema_for!(PlanArchiveRequest)),
        ("PlanCancelRequest", schema_for!(PlanCancelRequest)),
        ("PlanCompleteRequest", schema_for!(PlanCompleteRequest)),
        ("PlanCoverageGap", schema_for!(PlanCoverageGap)),
        ("PlanCreateRequest", schema_for!(PlanCreateRequest)),
        ("PlanCycle", schema_for!(PlanCycle)),
        ("PlanDiagnosticsQuery", schema_for!(PlanDiagnosticsQuery)),
        (
            "PlanDiagnosticsResponse",
            schema_for!(PlanDiagnosticsResponse),
        ),
        ("PlanEdge", schema_for!(PlanEdge)),
        ("PlanEdgeAddRequest", schema_for!(PlanEdgeAddRequest)),
        ("PlanEdgeRemoveRequest", schema_for!(PlanEdgeRemoveRequest)),
        ("PlanGetQuery", schema_for!(PlanGetQuery)),
        ("PlanGetResponse", schema_for!(PlanGetResponse)),
        ("PlanInvalidProfile", schema_for!(PlanInvalidProfile)),
        ("PlanListQuery", schema_for!(PlanListQuery)),
        ("PlanListResponse", schema_for!(PlanListResponse)),
        ("PlanRecord", schema_for!(PlanRecord)),
        ("PlanReplanRequest", schema_for!(PlanReplanRequest)),
        ("PlanSpecAddRequest", schema_for!(PlanSpecAddRequest)),
        ("PlanSpecMoveRequest", schema_for!(PlanSpecMoveRequest)),
        ("PlanSpecRemoveRequest", schema_for!(PlanSpecRemoveRequest)),
        ("PlanState", schema_for!(PlanState)),
        ("PlanVersionRecord", schema_for!(PlanVersionRecord)),
        ("SpecContent", schema_for!(SpecContent)),
        ("SpecContentState", schema_for!(SpecContentState)),
        (
            "SpecContentUpdateRequest",
            schema_for!(SpecContentUpdateRequest),
        ),
        ("SpecCreateRequest", schema_for!(SpecCreateRequest)),
        (
            "SpecExecutionMoveRequest",
            schema_for!(SpecExecutionMoveRequest),
        ),
        ("SpecExecutionState", schema_for!(SpecExecutionState)),
        ("SpecGetQuery", schema_for!(SpecGetQuery)),
        ("SpecGetResponse", schema_for!(SpecGetResponse)),
        ("SpecListQuery", schema_for!(SpecListQuery)),
        ("SpecListResponse", schema_for!(SpecListResponse)),
        ("SpecPlanJoinRequest", schema_for!(SpecPlanJoinRequest)),
        ("SpecRecord", schema_for!(SpecRecord)),
        (
            "SpecVersionApproveRequest",
            schema_for!(SpecVersionApproveRequest),
        ),
        ("SpecVersionGetQuery", schema_for!(SpecVersionGetQuery)),
        ("SpecVersionRecord", schema_for!(SpecVersionRecord)),
        (
            "SpecVersionSupersedeRequest",
            schema_for!(SpecVersionSupersedeRequest),
        ),
        ("TicketBugFactsRequest", schema_for!(TicketBugFactsRequest)),
        (
            "TicketBugQualification",
            schema_for!(TicketBugQualification),
        ),
        (
            "TicketBugQualifyRequest",
            schema_for!(TicketBugQualifyRequest),
        ),
        ("TicketBugRecord", schema_for!(TicketBugRecord)),
        ("TicketCreateRequest", schema_for!(TicketCreateRequest)),
        ("TicketCriterion", schema_for!(TicketCriterion)),
        (
            "TicketDependencyAddRequest",
            schema_for!(TicketDependencyAddRequest),
        ),
        (
            "TicketDependencyRecord",
            schema_for!(TicketDependencyRecord),
        ),
        (
            "TicketDependencyRemoveRequest",
            schema_for!(TicketDependencyRemoveRequest),
        ),
        (
            "TicketDependenciesQuery",
            schema_for!(TicketDependenciesQuery),
        ),
        (
            "TicketDependenciesResponse",
            schema_for!(TicketDependenciesResponse),
        ),
        (
            "TicketExternalReference",
            schema_for!(TicketExternalReference),
        ),
        ("TicketGetQuery", schema_for!(TicketGetQuery)),
        ("TicketKind", schema_for!(TicketKind)),
        ("TicketListQuery", schema_for!(TicketListQuery)),
        ("TicketListResponse", schema_for!(TicketListResponse)),
        (
            "TicketOccurrenceSnapshot",
            schema_for!(TicketOccurrenceSnapshot),
        ),
        ("TicketPriority", schema_for!(TicketPriority)),
        (
            "TicketReadinessBlocker",
            schema_for!(TicketReadinessBlocker),
        ),
        ("TicketReadinessQuery", schema_for!(TicketReadinessQuery)),
        (
            "TicketReadinessResponse",
            schema_for!(TicketReadinessResponse),
        ),
        ("TicketRecord", schema_for!(TicketRecord)),
        ("TicketSeverity", schema_for!(TicketSeverity)),
        ("TicketState", schema_for!(TicketState)),
        (
            "TicketBlockerAddRequest",
            schema_for!(TicketBlockerAddRequest),
        ),
        ("TicketBlockerRecord", schema_for!(TicketBlockerRecord)),
        (
            "TicketBlockerRemoveRequest",
            schema_for!(TicketBlockerRemoveRequest),
        ),
        (
            "TicketVerificationStep",
            schema_for!(TicketVerificationStep),
        ),
        ("ProjectArchiveRequest", schema_for!(ProjectArchiveRequest)),
        ("ProjectCounters", schema_for!(ProjectCounters)),
        ("ProjectListQuery", schema_for!(ProjectListQuery)),
        ("ProjectListResponse", schema_for!(ProjectListResponse)),
        ("ProjectRecord", schema_for!(ProjectRecord)),
        (
            "ProjectRegisterRequest",
            schema_for!(ProjectRegisterRequest),
        ),
        ("DeferralListQuery", schema_for!(DeferralListQuery)),
        ("DeferralListResponse", schema_for!(DeferralListResponse)),
        ("DeferralRecord", schema_for!(DeferralRecord)),
        ("DeferralRecordRequest", schema_for!(DeferralRecordRequest)),
        (
            "DeferralSupersedeRequest",
            schema_for!(DeferralSupersedeRequest),
        ),
        ("RulingIdentity", schema_for!(RulingIdentity)),
        ("RefusedCriterion", schema_for!(RefusedCriterion)),
        (
            "SpecCoverageCheckQuery",
            schema_for!(SpecCoverageCheckQuery),
        ),
        (
            "SpecCoverageCheckResponse",
            schema_for!(SpecCoverageCheckResponse),
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
        ("WorkspaceCheckoutDto", schema_for!(WorkspaceCheckoutDto)),
        ("WorkspaceHealthDto", schema_for!(WorkspaceHealthDto)),
        ("WorkspaceListQuery", schema_for!(WorkspaceListQuery)),
        ("WorkspaceListResponse", schema_for!(WorkspaceListResponse)),
        (
            "WorkspaceObserveRequest",
            schema_for!(WorkspaceObserveRequest),
        ),
        (
            "WorkspaceObservationDto",
            schema_for!(WorkspaceObservationDto),
        ),
        ("WorkspaceRecord", schema_for!(WorkspaceRecord)),
        (
            "WorkspaceRegisterRequest",
            schema_for!(WorkspaceRegisterRequest),
        ),
        (
            "WorkspaceRetireRequest",
            schema_for!(WorkspaceRetireRequest),
        ),
        ("WorkspaceReuseDto", schema_for!(WorkspaceReuseDto)),
    ]
}
