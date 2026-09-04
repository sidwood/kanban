//! Named application commands and queries shared by the UI and every
//! other client, with the ports they call through.

pub mod catalog;
pub mod contracts_gen;
pub mod dispatch;
pub mod events;
pub mod initiative;
pub mod mutation;
pub mod timeline;

pub use catalog::{OperationDescriptor, OperationKind, exposed_operations};
pub use dispatch::{Core, QueryHandler, RegistrationError};
pub use events::{EventSink, NoopEventSink};
pub use initiative::{InitiativeStore, TimelineAppend};
pub use mutation::{
    CommandHandler, IdempotencyStore, MemoryIdempotencyStore, ParsedCommand, RecordedOutcome,
    parse_payload,
};
pub use timeline::{
    TimelineError, TimelineQueryHandler, TimelineRecorder, TimelineStore, entity_kind_wire,
    event_kind_wire,
};

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::exposed_operations;

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
