//! Pure domain rules: entities, closed state vocabularies, transition
//! rules, and invariants. No I/O, no async, no DTO types, no clock.

pub mod board;
pub mod board_query;
pub mod capability;
pub mod capacity;
pub mod clone;
pub mod comment;
pub mod coverage;
pub mod deferral;
pub mod dependency;
pub mod dispatch;
pub mod evidence;
pub mod graph_proposal;
pub mod herdr;
pub mod initiative;
pub mod lane;
pub mod lifecycle;
pub mod plan;
pub mod profile;
pub mod project;
pub mod reassignment;
pub mod ruling;
pub mod run;
pub mod saved_view;
pub mod schedule;
pub mod spec;
pub mod ticket;
pub mod timeline;
pub mod timeline_time;
pub mod workspace;

pub use board::{BoardGroup, board_group_for};
pub use board_query::{AttentionState, BoardCard, BoardFilter, admits, compare_cards, sort_cards};
pub use capability::{
    Capability, CapabilityError, CapabilityId, CapabilityRefusal, CapabilityRole, CapabilityScope,
    CapabilityStatus, McpOperations, ReviewerSlotId, enforce_within_surface,
};
pub use capacity::{
    ActiveRun, CapacityError, CapacityInputs, CapacityRefusal, GlobalCapacity, ProjectCapacity,
    evaluate_capacity,
};
pub use clone::{
    CloneConflict, CloneTargetError, WorkspaceCloneFacts, clone_create_conflict,
    clone_remove_conflict, validate_clone_target,
};
pub use comment::{
    Comment, CommentError, CommentId, CommentRevision, CommentTarget, CommentText, TextError,
};
pub use coverage::{
    AcceptanceCriterion, CriterionError, ExecutableRefusal, ScopeError, StoryRefError, StoryScope,
    UserStoryRef, VerificationStep, VerificationStepError, enforce_executable,
};
pub use deferral::{Deferral, DeferralDraft, DeferralError, DeferralId, DeferralReason};
pub use dependency::{
    BlockerDescription, DependencyError, DependencyState, ExternalBlocker, ExternalBlockerId,
    Readiness, ReadinessBlocker, ReadinessInputs, TicketDependency, TicketDependencyGraph,
    compute_readiness, dependency_satisfied,
};
pub use dispatch::{
    ClaimDecision, DispatchError, DispatchRequest, DispatchRequestId, DispatchStatus,
    compare_queue, decide_claim, refuse_duplicate_open, sort_queue,
};
pub use evidence::{
    CommitIdentity, ContentHash, EvidenceError, EvidenceId, EvidenceItem, EvidenceKind,
    EvidenceShape, RelativePath,
};
pub use graph_proposal::{
    GraphApprovalRefusal, GraphProposalError, GraphProposalId, GraphProposalState,
    TicketGraphProposal, claims_count_for_version, enforce_acyclic_with_registered,
    enforce_approvable, enforce_assignable, enforce_executable_member,
};
pub use herdr::{HerdrSession, validate_herdr_session_name};
pub use initiative::{
    Initiative, InitiativeError, InitiativeId, InitiativeName, InitiativeState, NameError,
};
pub use lane::{Lane, LaneError, LaneId, workspace_lane_conflict};
pub use lifecycle::{
    Actor, HumanCommand, LifecycleError, OverrideJustification, ReviewDecision, apply_command,
    apply_drag, apply_override, human_may_drag, legal_targets,
};
pub use plan::{
    DependencyCycle, DependencyEdge, Plan, PlanError, PlanId, PlanShape, PlanState, PlanVersion,
    SpecNumber, SpecNumberError,
};
pub use profile::{
    ExecutionProfile, ProfileCatalogue, ProfileDefinition, ProfileError, ProfileName, ProfileState,
};
pub use project::{
    CodeError, NumberKind, Project, ProjectCode, ProjectCounters, ProjectError, ProjectId,
    ProjectRegistration, ProjectState, RegistrationError,
};
pub use reassignment::{ReassignmentError, apply_reassignment};
pub use ruling::{Ruling, RulingDraft, RulingEntityRef, RulingError, RulingId, RulingSummary};
pub use run::{ProfileSnapshot, Run, RunError, RunId, RunStatus, resolve_effective};
pub use saved_view::{
    DEFAULT_VIEW_NAME, DonePlacement, EXPANDABLE_GROUPS, SavedView, SavedViewError, SavedViewId,
    ViewMode, ViewName, ViewScope, ViewSorting,
};
pub use schedule::{
    Activation, CronExpression, Schedule, ScheduleError, ScheduleId, ScheduleState,
    ScheduleTrigger, Timezone, accepts, stored_instant_of,
};
pub use spec::{
    ContentChange, Spec, SpecContent, SpecContentState, SpecError, SpecExecutionState, SpecId,
    SpecVersion,
};
pub use ticket::{
    BugFacts, BugQualification, BugTicket, CompletionCriterion, ExternalReference,
    ImplementationTicket, OccurrenceSnapshot, Priority, Severity, SnapshotError, TaskMode,
    TaskSubtype, TaskTicket, TaskTiming, Ticket, TicketBody, TicketError, TicketId, TicketKind,
    TicketNumber, TicketNumberError, TicketState,
};
pub use timeline::{ENTITY_KINDS, is_entity_kind};
pub use timeline_time::{
    TimelineTimeError, normalise_timeline_bound, validate_timeline_time_window,
};
pub use workspace::{
    ReuseEvaluation, ReuseInputs, Workspace, WorkspaceCheckout, WorkspaceHealth,
    WorkspaceHealthInputs, WorkspaceId, WorkspaceObservation, WorkspaceRegistration,
    WorkspaceRegistrationError, WorkspaceRetirementError, compute_health, evaluate_reuse,
};
