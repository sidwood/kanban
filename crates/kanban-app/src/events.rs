//! The port commands use to publish ordered events.

use serde::Serialize;
use serde_json::Value;

use crate::event_catalog::EventDescriptor;

/// Where commands publish events. The transport's broker implements
/// this, assigning sequence numbers and delivering to every
/// subscriber in order; the application layer only says what
/// happened.
pub trait EventSink: Send + Sync {
    /// Publish one event.
    fn emit(&self, event_type: &str, payload: Value);
}

/// Publish one catalogued live event with a typed payload.
pub fn emit_catalogued(sink: &dyn EventSink, event: &EventDescriptor, payload: &impl Serialize) {
    let payload = serde_json::to_value(payload).expect("catalogued payload serialises");
    sink.emit(event.name, payload);
}

/// An [`EventSink`] that discards events, for cores that run without
/// a broker.
#[derive(Debug, Default)]
pub struct NoopEventSink;

impl EventSink for NoopEventSink {
    fn emit(&self, _event_type: &str, _payload: Value) {}
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use kanban_dto::InitiativeRecord;
    use serde_json::{Value, json};

    use super::{EventSink, emit_catalogued};
    use crate::event_catalog::exposed_events;

    #[derive(Debug, Default)]
    struct RecordingSink {
        events: Mutex<Vec<(String, Value)>>,
    }

    impl EventSink for RecordingSink {
        fn emit(&self, event_type: &str, payload: Value) {
            self.events
                .lock()
                .expect("the recorder lock is sound")
                .push((event_type.to_owned(), payload));
        }
    }

    #[test]
    fn catalogued_emits_use_the_descriptor_name() {
        let sink = RecordingSink::default();
        let event = exposed_events()
            .iter()
            .find(|descriptor| descriptor.name == "initiative.created")
            .expect("the catalogue lists initiative.created");
        let payload = InitiativeRecord {
            id: 1,
            name: "Alpha".to_owned(),
            archived: false,
            version: 1,
        };

        emit_catalogued(&sink, event, &payload);

        let events = sink.events.lock().expect("the recorder lock is sound");
        assert_eq!(
            *events,
            vec![(
                "initiative.created".to_owned(),
                json!({
                    "id": 1,
                    "name": "Alpha",
                    "archived": false,
                    "version": 1,
                })
            )]
        );
    }
}
