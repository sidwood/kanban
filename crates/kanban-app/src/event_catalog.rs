//! Re-export the authoritative live-event catalogue from `kanban-dto`.

pub use kanban_dto::{
    LiveEventDescriptor as EventDescriptor, LiveEventName, event_descriptor, live_event_catalog,
};

/// Live events the application layer currently publishes to every
/// subscriber.
pub fn exposed_events() -> &'static [EventDescriptor] {
    live_event_catalog()
}
