//! Pure domain rules: entities, closed state vocabularies, transition
//! rules, and invariants. No I/O, no async, no DTO types, no clock.

pub mod initiative;
pub mod timeline;

pub use initiative::{
    Initiative, InitiativeError, InitiativeId, InitiativeName, InitiativeState, NameError,
};
pub use timeline::{
    ENTITY_KINDS, EVENT_KINDS, TimelineEntityKind, TimelineEventKind, is_entity_kind, is_event_kind,
};
