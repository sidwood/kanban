//! Pure domain rules: entities, closed state vocabularies, transition
//! rules, and invariants. No I/O, no async, no DTO types, no clock.

pub mod comment;
pub mod deferral;
pub mod evidence;
pub mod initiative;
pub mod project;
pub mod ruling;
pub mod timeline;

pub use comment::{
    Comment, CommentError, CommentId, CommentRevision, CommentTarget, CommentText, TextError,
};
pub use deferral::{Deferral, DeferralDraft, DeferralError, DeferralId, DeferralReason};
pub use evidence::{
    CommitIdentity, ContentHash, EvidenceError, EvidenceId, EvidenceItem, EvidenceKind,
    EvidenceShape, RelativePath,
};
pub use initiative::{
    Initiative, InitiativeError, InitiativeId, InitiativeName, InitiativeState, NameError,
};
pub use project::{
    CodeError, NumberKind, Project, ProjectCode, ProjectCounters, ProjectError, ProjectId,
    ProjectRegistration, ProjectState, RegistrationError,
};
pub use ruling::{Ruling, RulingDraft, RulingEntityRef, RulingError, RulingId, RulingSummary};
pub use timeline::{ENTITY_KINDS, is_entity_kind};
