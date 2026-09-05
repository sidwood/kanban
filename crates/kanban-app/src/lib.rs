//! Named application commands and queries shared by the UI and every
//! other client, with the ports they call through.

pub mod catalog;
pub mod comment;
pub mod contracts_gen;
pub mod deferrals;
pub mod dispatch;
pub mod event_catalog;
pub mod events;
pub mod evidence;
pub mod herdr;
pub mod initiative;
pub mod mutation;
pub mod project;
pub mod rulings;
pub mod timeline;

pub use catalog::{
    EXPOSED_MCP_TOOL_NAMES, EXPOSED_OPERATION_COUNT, OperationDescriptor, OperationKind,
    assert_registered_matches_exposed_catalogue, exposed_operations,
};
pub use comment::CommentStore;
pub use deferrals::DeferralStore;
pub use dispatch::{Core, QueryHandler, RegistrationError};
pub use event_catalog::{EventDescriptor, exposed_events};
pub use events::{EventSink, NoopEventSink, emit_catalogued};
pub use evidence::{EvidenceFilter, EvidenceStore};
pub use herdr::{HerdrDiagnostics, HerdrSettingsStore};
pub use initiative::InitiativeStore;
#[cfg(any(test, feature = "test-support"))]
pub use mutation::MemoryIdempotencyStore;
pub use mutation::{
    CommandHandler, IdempotencyStore, MutationSpan, ParsedCommand, RecordedOutcome, parse_payload,
};
pub use project::{GitObservation, ProjectStore, duplicate_code_error, duplicate_session_error};
pub use rulings::RulingStore;
pub use timeline::{
    TimelineEnvelope, TimelineError, TimelineFacts, TimelineQueryHandler, TimelineStore,
};

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::{exposed_events, exposed_operations};

    #[test]
    fn exposed_events_reference_known_dto_schemas() {
        let known_schemas: HashSet<_> = kanban_dto::schema_definitions()
            .into_iter()
            .map(|(name, _)| name)
            .collect();

        for event in exposed_events() {
            assert!(
                known_schemas.contains(event.payload_schema),
                "unknown payload schema `{}` for `{}`",
                event.payload_schema,
                event.name
            );
        }
    }

    #[test]
    fn exposed_event_names_are_unique() {
        let mut seen = HashSet::new();

        for event in exposed_events() {
            assert!(
                seen.insert(event.name),
                "duplicate event name `{}`",
                event.name
            );
        }
    }

    #[test]
    fn exposed_events_match_the_live_event_name_catalogue() {
        let mut names: Vec<_> = exposed_events().iter().map(|event| event.name).collect();
        names.sort_unstable();

        let mut catalogue: Vec<_> = [
            kanban_dto::LiveEventName::InitiativeCreated,
            kanban_dto::LiveEventName::InitiativeRenamed,
            kanban_dto::LiveEventName::InitiativeArchived,
            kanban_dto::LiveEventName::ProjectRegistered,
            kanban_dto::LiveEventName::ProjectArchived,
            kanban_dto::LiveEventName::CommentCreated,
            kanban_dto::LiveEventName::CommentEdited,
            kanban_dto::LiveEventName::RulingRecorded,
            kanban_dto::LiveEventName::RulingSuperseded,
            kanban_dto::LiveEventName::DeferralRecorded,
            kanban_dto::LiveEventName::DeferralSuperseded,
            kanban_dto::LiveEventName::EvidenceAttached,
            kanban_dto::LiveEventName::EvidenceListed,
        ]
        .into_iter()
        .map(kanban_dto::LiveEventName::as_str)
        .collect();
        catalogue.sort_unstable();

        assert_eq!(names, catalogue, "the catalogues must agree");
    }

    #[test]
    fn exposed_operations_reference_known_dto_schemas() {
        let known_schemas: HashSet<_> = kanban_dto::schema_definitions()
            .into_iter()
            .map(|(name, _)| name)
            .collect();

        for operation in exposed_operations() {
            assert!(
                known_schemas.contains(operation.request_schema),
                "unknown request schema `{}` for `{}`",
                operation.request_schema,
                operation.name
            );
            assert!(
                known_schemas.contains(operation.response_schema),
                "unknown response schema `{}` for `{}`",
                operation.response_schema,
                operation.name
            );
        }
    }

    #[test]
    fn exposed_operation_names_are_unique() {
        let mut seen = HashSet::new();

        for operation in exposed_operations() {
            assert!(
                seen.insert(operation.name),
                "duplicate operation name `{}`",
                operation.name
            );
        }
    }
}
