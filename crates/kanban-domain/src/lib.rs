//! Pure domain rules: entities, closed state vocabularies, transition
//! rules, and invariants. No I/O, no async, no DTO types, no clock.

pub mod comment;
pub mod coverage;
pub mod deferral;
pub mod evidence;
pub mod herdr;
pub mod initiative;
pub mod plan;
pub mod project;
pub mod ruling;
pub mod spec;
pub mod ticket;
pub mod timeline;
pub mod timeline_time;
pub mod workspace;

pub use comment::{
    Comment, CommentError, CommentId, CommentRevision, CommentTarget, CommentText, TextError,
};
pub use coverage::{
    AcceptanceCriterion, CriterionError, ExecutableRefusal, ScopeError, StoryRefError, StoryScope,
    UserStoryRef, VerificationStep, VerificationStepError, enforce_executable,
};
pub use deferral::{Deferral, DeferralDraft, DeferralError, DeferralId, DeferralReason};
pub use evidence::{
    CommitIdentity, ContentHash, EvidenceError, EvidenceId, EvidenceItem, EvidenceKind,
    EvidenceShape, RelativePath,
};
pub use herdr::{HerdrSession, validate_herdr_session_name};
pub use initiative::{
    Initiative, InitiativeError, InitiativeId, InitiativeName, InitiativeState, NameError,
};
pub use plan::{
    DependencyCycle, DependencyEdge, Plan, PlanError, PlanId, PlanShape, PlanState, PlanVersion,
    SpecNumber, SpecNumberError, cycles_in,
};
pub use project::{
    CodeError, NumberKind, Project, ProjectCode, ProjectCounters, ProjectError, ProjectId,
    ProjectRegistration, ProjectState, RegistrationError,
};
pub use ruling::{Ruling, RulingDraft, RulingEntityRef, RulingError, RulingId, RulingSummary};
pub use spec::{
    ContentChange, Spec, SpecContent, SpecContentState, SpecError, SpecExecutionState, SpecId,
    SpecVersion,
};
pub use ticket::{
    BugTicket, ImplementationTicket, Priority, TaskTicket, Ticket, TicketBody, TicketError,
    TicketId, TicketKind, TicketNumber, TicketNumberError, TicketState,
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
