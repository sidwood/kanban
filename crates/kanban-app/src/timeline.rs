//! The typed timeline envelope, the query port, and the query
//! handler (KAN-S2).

use kanban_dto::{
    ApiError, TimelineEntityRef, TimelineEventKind, TimelineEventRecord, TimelineQuery,
    TimelineQueryResponse, TimelineScope,
};

use serde_json::Value;

use crate::dispatch::QueryHandler;
use crate::mutation::parse_payload;

/// One durable timeline row as the application layer states it:
/// where it belongs, which closed event kind it is, the entity it
/// is about, and the structured facts. Storage inserts it into the
/// entity's own transaction unchanged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimelineEnvelope {
    scope: TimelineScope,
    kind: TimelineEventKind,
    entity: Option<TimelineEntityRef>,
    detail: Value,
}

impl TimelineEnvelope {
    /// An event about an entity that sits above every Project.
    pub fn global(
        kind: TimelineEventKind,
        entity: Option<TimelineEntityRef>,
        detail: Value,
    ) -> Self {
        Self {
            scope: TimelineScope::Global,
            kind,
            entity,
            detail,
        }
    }

    /// An event inside one Project. The scope is the Project's
    /// numeric identity, already resolved through the Project store;
    /// every project-scoped writer derives its row from this one
    /// constructor, so one Project owns exactly one timeline scope.
    pub fn project(
        project_id: u64,
        kind: TimelineEventKind,
        entity: Option<TimelineEntityRef>,
        detail: Value,
    ) -> Self {
        Self {
            scope: TimelineScope::Project(project_id),
            kind,
            entity,
            detail,
        }
    }

    /// Where the row belongs.
    pub fn scope(&self) -> &TimelineScope {
        &self.scope
    }

    /// The closed event kind.
    pub fn kind(&self) -> TimelineEventKind {
        self.kind
    }

    /// The entity the event is about, when it has one.
    pub fn entity(&self) -> Option<&TimelineEntityRef> {
        self.entity.as_ref()
    }

    /// The structured facts of the change.
    pub fn detail(&self) -> &Value {
        &self.detail
    }
}

/// The facts of one change that only becomes an envelope once
/// storage has minted the entity's identity.
#[derive(Debug, Clone)]
pub struct TimelineFacts {
    /// The closed event kind.
    pub kind: TimelineEventKind,
    /// The change's facts, e.g. a Ruling's summary.
    pub facts: Value,
}

/// The storage port for reading the append-only timeline. Rows are
/// written through the entity stores, inside the transaction that
/// changes the entity, so this port never appends.
pub trait TimelineStore: Send + Sync {
    /// Query events in one scope with optional filters.
    fn query(&self, query: &TimelineQuery) -> Result<Vec<TimelineEventRecord>, TimelineError>;
}

/// Why the timeline port refused an operation.
#[derive(Debug, PartialEq, Eq)]
pub enum TimelineError {
    /// The inputs were invalid.
    Invalid(String),
    /// The storage layer failed.
    Storage(String),
}

impl<S: TimelineStore + ?Sized> TimelineStore for &S {
    fn query(&self, query: &TimelineQuery) -> Result<Vec<TimelineEventRecord>, TimelineError> {
        (*self).query(query)
    }
}

impl<T: TimelineStore + ?Sized> TimelineStore for std::sync::Arc<T> {
    fn query(&self, query: &TimelineQuery) -> Result<Vec<TimelineEventRecord>, TimelineError> {
        self.as_ref().query(query)
    }
}

/// Serves `timeline.query`.
pub struct TimelineQueryHandler<S: TimelineStore> {
    store: S,
}

impl<S: TimelineStore> TimelineQueryHandler<S> {
    /// Builds the handler around `store`.
    pub fn new(store: S) -> Self {
        Self { store }
    }
}

impl<S: TimelineStore + 'static> QueryHandler for TimelineQueryHandler<S> {
    fn handle(&self, payload: &Value) -> Result<Value, ApiError> {
        let query = parse_payload::<TimelineQuery>(payload)?;
        let events = self.store.query(&query).map_err(map_timeline_error)?;
        let response = TimelineQueryResponse { events };
        serde_json::to_value(response).map_err(|error| ApiError::internal(&error.to_string()))
    }
}

fn map_timeline_error(error: TimelineError) -> ApiError {
    match error {
        TimelineError::Invalid(reason) => ApiError::invalid_request(&reason),
        TimelineError::Storage(reason) => ApiError::internal(&reason),
    }
}

#[cfg(test)]
mod envelope {
    use kanban_dto::{TimelineEntityKind, TimelineEntityRef, TimelineEventKind};
    use serde_json::json;

    use super::TimelineEnvelope;
    use kanban_dto::TimelineScope;

    fn initiative_entity() -> TimelineEntityRef {
        TimelineEntityRef {
            kind: TimelineEntityKind::Initiative,
            id: "1".to_owned(),
        }
    }

    #[test]
    fn a_global_envelope_records_above_every_project() {
        let envelope = TimelineEnvelope::global(
            TimelineEventKind::Transition,
            Some(initiative_entity()),
            json!({ "action": "created" }),
        );

        assert_eq!(envelope.scope(), &TimelineScope::Global);
        assert_eq!(envelope.kind(), TimelineEventKind::Transition);
        assert_eq!(envelope.entity(), Some(&initiative_entity()));
        assert_eq!(envelope.detail(), &json!({ "action": "created" }));
    }

    #[test]
    fn a_project_envelope_carries_its_project_identity() {
        let envelope = TimelineEnvelope::project(
            1,
            TimelineEventKind::Comment,
            None,
            json!({ "text": "noted" }),
        );

        assert_eq!(envelope.scope(), &TimelineScope::Project(1));
    }
}

#[cfg(test)]
mod vocabulary {
    use kanban_dto::TimelineEntityKind;

    #[test]
    fn the_domain_and_the_wire_name_the_same_entity_kinds() {
        let wire: Vec<&str> = TimelineEntityKind::ALL
            .iter()
            .map(TimelineEntityKind::as_str)
            .collect();

        assert_eq!(
            wire,
            kanban_domain::ENTITY_KINDS.to_vec(),
            "domain rules and payload definitions must refuse the same entity kinds"
        );
    }
}

#[cfg(test)]
mod timeline_query {
    use std::sync::Mutex;

    use kanban_dto::{
        TimelineEntityKind, TimelineEntityRef, TimelineEventKind, TimelineEventRecord,
        TimelineQuery, TimelineScope,
    };
    use serde_json::{Value, json};

    use super::{TimelineEnvelope, TimelineQueryHandler, TimelineStore};
    use crate::dispatch::{Core, QueryHandler};
    use crate::events::NoopEventSink;
    use crate::mutation::MemoryIdempotencyStore;

    /// The rows one scope already holds. Production rows are written
    /// by the entity stores, so this fixture seeds them directly.
    #[derive(Default)]
    struct MemoryTimelineStore {
        events: Mutex<Vec<TimelineEventRecord>>,
    }

    impl MemoryTimelineStore {
        fn seed(&self, envelope: TimelineEnvelope) {
            let mut events = self.events.lock().expect("timeline lock is sound");
            let id = events.len() as u64 + 1;
            events.push(TimelineEventRecord {
                id,
                scope: *envelope.scope(),
                kind: envelope.kind(),
                entity: envelope.entity().cloned(),
                recorded_at: format!("2026-09-04T12:00:{id:02}Z"),
                detail: envelope.detail().clone(),
            });
        }
    }

    impl TimelineStore for MemoryTimelineStore {
        fn query(
            &self,
            query: &TimelineQuery,
        ) -> Result<Vec<TimelineEventRecord>, super::TimelineError> {
            let events = self.events.lock().expect("timeline lock is sound");
            Ok(events
                .iter()
                .filter(|event| event.scope == query.scope)
                .filter(|event| {
                    query
                        .entity
                        .as_ref()
                        .map(|entity| event.entity.as_ref() == Some(entity))
                        .unwrap_or(true)
                })
                .filter(|event| {
                    query
                        .kinds
                        .as_ref()
                        .map(|kinds| kinds.contains(&event.kind))
                        .unwrap_or(true)
                })
                .filter(|event| {
                    query
                        .since
                        .as_ref()
                        .map(|since| event.recorded_at.as_str() >= since.as_str())
                        .unwrap_or(true)
                })
                .filter(|event| {
                    query
                        .until
                        .as_ref()
                        .map(|until| event.recorded_at.as_str() <= until.as_str())
                        .unwrap_or(true)
                })
                .cloned()
                .collect())
        }
    }

    fn ticket_entity() -> TimelineEntityRef {
        TimelineEntityRef {
            kind: TimelineEntityKind::Ticket,
            id: "kan-t9".to_owned(),
        }
    }

    fn project_event(
        kind: TimelineEventKind,
        entity: TimelineEntityRef,
        detail: Value,
    ) -> TimelineEnvelope {
        TimelineEnvelope::project(1, kind, Some(entity), detail)
    }

    #[test]
    fn timeline_query_filters_by_entity_kind_and_time() {
        let store = std::sync::Arc::new(MemoryTimelineStore::default());
        store.seed(project_event(
            TimelineEventKind::Transition,
            ticket_entity(),
            json!({ "to": "in_progress" }),
        ));
        store.seed(project_event(
            TimelineEventKind::Comment,
            TimelineEntityRef {
                kind: TimelineEntityKind::Ticket,
                id: "kan-t10".to_owned(),
            },
            json!({ "text": "elsewhere" }),
        ));

        let handler = TimelineQueryHandler::new(store);
        let response = handler
            .handle(&json!({
                "scope": { "project": 1 },
                "entity": ticket_entity(),
                "kinds": ["transition"],
                "since": "2026-09-04T12:00:00Z",
                "until": "2026-09-04T12:00:02Z",
            }))
            .expect("query serves");

        assert_eq!(
            response,
            json!({
                "events": [{
                    "id": 1,
                    "scope": { "project": 1 },
                    "kind": "transition",
                    "entity": ticket_entity(),
                    "recorded_at": "2026-09-04T12:00:01Z",
                    "detail": { "to": "in_progress" },
                }]
            })
        );
    }

    #[test]
    fn the_global_scope_serves_events_above_every_project() {
        let store = std::sync::Arc::new(MemoryTimelineStore::default());
        store.seed(TimelineEnvelope::global(
            TimelineEventKind::Transition,
            Some(TimelineEntityRef {
                kind: TimelineEntityKind::Initiative,
                id: "1".to_owned(),
            }),
            json!({ "action": "created", "id": 1 }),
        ));
        store.seed(project_event(
            TimelineEventKind::Transition,
            ticket_entity(),
            json!({ "to": "in_progress" }),
        ));

        let handler = TimelineQueryHandler::new(store);
        let response = handler
            .handle(&json!({ "scope": "global" }))
            .expect("the global scope serves");

        assert_eq!(
            response,
            json!({
                "events": [{
                    "id": 1,
                    "scope": "global",
                    "kind": "transition",
                    "entity": { "kind": "initiative", "id": "1" },
                    "recorded_at": "2026-09-04T12:00:01Z",
                    "detail": { "action": "created", "id": 1 },
                }]
            }),
            "Initiative history is reachable and no Project row leaks in"
        );
    }

    #[test]
    fn timeline_query_rejects_unknown_fields() {
        let store = std::sync::Arc::new(MemoryTimelineStore::default());
        let handler = TimelineQueryHandler::new(store);

        let error = handler
            .handle(&json!({ "scope": "global", "surprise": true }))
            .expect_err("unknown fields are rejected");

        assert_eq!(error.code, kanban_dto::ErrorCode::UnknownField);
    }

    #[test]
    fn timeline_query_is_catalogued_on_the_core() {
        let store = std::sync::Arc::new(MemoryTimelineStore::default());
        let mut core = Core::new(
            crate::catalog::exposed_operations(),
            std::sync::Arc::new(MemoryIdempotencyStore::new()),
            std::sync::Arc::new(NoopEventSink),
        );
        core.register_query(
            "timeline.query",
            std::sync::Arc::new(TimelineQueryHandler::new(store)),
        )
        .expect("timeline query registers");

        let response = core
            .query(
                "timeline.query",
                &json!({ "scope": { "project": 1 }, "kinds": ["transition"] }),
            )
            .expect("the core serves timeline queries");

        assert_eq!(response, json!({ "events": [] }));
    }

    #[test]
    fn every_event_kind_can_be_carried_by_an_envelope() {
        let store = std::sync::Arc::new(MemoryTimelineStore::default());
        for (index, kind) in TimelineEventKind::ALL.iter().enumerate() {
            store.seed(project_event(
                *kind,
                ticket_entity(),
                json!({ "sequence": index }),
            ));
        }

        let events = store
            .query(&TimelineQuery {
                scope: TimelineScope::Project(1),
                entity: None,
                kinds: None,
                since: None,
                until: None,
            })
            .expect("query succeeds");

        assert_eq!(
            events.iter().map(|event| event.kind).collect::<Vec<_>>(),
            TimelineEventKind::ALL.to_vec(),
            "the closed vocabulary reaches the timeline whole"
        );
    }
}
