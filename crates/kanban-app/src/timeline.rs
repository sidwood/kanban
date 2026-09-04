//! The timeline port, recorder, and query handler (KAN-S2).

use kanban_dto::{
    ApiError, TimelineEntityRef, TimelineEventKind, TimelineEventRecord, TimelineQuery,
    TimelineQueryResponse,
};
use serde_json::Value;

use crate::dispatch::QueryHandler;
use crate::mutation::parse_payload;

/// The storage port for the append-only per-Project timeline.
pub trait TimelineStore: Send + Sync {
    /// Append one typed event to a Project timeline.
    fn append(
        &self,
        project_id: &str,
        kind: TimelineEventKind,
        entity: Option<TimelineEntityRef>,
        detail: Value,
    ) -> Result<(), TimelineError>;

    /// Query events for one Project with optional filters.
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
    fn append(
        &self,
        project_id: &str,
        kind: TimelineEventKind,
        entity: Option<TimelineEntityRef>,
        detail: Value,
    ) -> Result<(), TimelineError> {
        (*self).append(project_id, kind, entity, detail)
    }

    fn query(&self, query: &TimelineQuery) -> Result<Vec<TimelineEventRecord>, TimelineError> {
        (*self).query(query)
    }
}

impl<T: TimelineStore + ?Sized> TimelineStore for std::sync::Arc<T> {
    fn append(
        &self,
        project_id: &str,
        kind: TimelineEventKind,
        entity: Option<TimelineEntityRef>,
        detail: Value,
    ) -> Result<(), TimelineError> {
        self.as_ref().append(project_id, kind, entity, detail)
    }

    fn query(&self, query: &TimelineQuery) -> Result<Vec<TimelineEventRecord>, TimelineError> {
        self.as_ref().query(query)
    }
}

/// Records domain transitions on the per-Project timeline.
pub struct TimelineRecorder<S: TimelineStore> {
    store: S,
}

impl<S: TimelineStore> TimelineRecorder<S> {
    /// Wraps `store` as the application's timeline writer.
    pub fn new(store: S) -> Self {
        Self { store }
    }

    /// Append one typed domain event after validating the kind.
    pub fn record(
        &self,
        project_id: &str,
        kind: &str,
        entity: Option<TimelineEntityRef>,
        detail: Value,
    ) -> Result<(), TimelineError> {
        let kind = TimelineEventKind::parse(kind).ok_or_else(|| {
            TimelineError::Invalid(format!("unknown timeline event kind `{kind}`"))
        })?;
        self.store.append(project_id, kind, entity, detail)
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
        TimelineQuery,
    };
    use serde_json::{Value, json};

    use super::{TimelineQueryHandler, TimelineRecorder, TimelineStore};
    use crate::dispatch::{Core, QueryHandler};
    use crate::events::NoopEventSink;
    use crate::mutation::MemoryIdempotencyStore;

    #[derive(Default)]
    struct MemoryTimelineStore {
        events: Mutex<Vec<TimelineEventRecord>>,
    }

    impl TimelineStore for MemoryTimelineStore {
        fn append(
            &self,
            project_id: &str,
            kind: TimelineEventKind,
            entity: Option<TimelineEntityRef>,
            detail: Value,
        ) -> Result<(), super::TimelineError> {
            let mut events = self.events.lock().expect("timeline lock is sound");
            let id = events.len() as u64 + 1;
            events.push(TimelineEventRecord {
                id,
                project_id: project_id.to_owned(),
                kind,
                entity,
                recorded_at: format!("2026-09-04T12:00:{id:02}Z"),
                detail,
            });
            Ok(())
        }

        fn query(
            &self,
            query: &TimelineQuery,
        ) -> Result<Vec<TimelineEventRecord>, super::TimelineError> {
            let events = self.events.lock().expect("timeline lock is sound");
            Ok(events
                .iter()
                .filter(|event| event.project_id == query.project_id)
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

    #[test]
    fn domain_transitions_append_every_required_event_kind() {
        let store = MemoryTimelineStore::default();
        let recorder = TimelineRecorder::new(&store);
        let entity = ticket_entity();

        for (index, kind) in TimelineEventKind::ALL.iter().enumerate() {
            recorder
                .record(
                    "kan",
                    kind.as_str(),
                    Some(entity.clone()),
                    json!({ "sequence": index }),
                )
                .expect("append succeeds");
        }

        let events = store
            .query(&TimelineQuery {
                project_id: "kan".to_owned(),
                entity: None,
                kinds: None,
                since: None,
                until: None,
            })
            .expect("query succeeds");
        assert_eq!(events.len(), TimelineEventKind::ALL.len());
        let kinds: Vec<_> = events.iter().map(|event| event.kind).collect();
        for kind in TimelineEventKind::ALL {
            assert!(kinds.contains(kind), "missing kind `{}`", kind.as_str());
        }
    }

    #[test]
    fn timeline_query_filters_by_entity_kind_and_time() {
        let store = std::sync::Arc::new(MemoryTimelineStore::default());
        let recorder = TimelineRecorder::new(store.clone());
        let entity = ticket_entity();
        recorder
            .record(
                "kan",
                "transition",
                Some(entity.clone()),
                json!({ "to": "in_progress" }),
            )
            .expect("first event lands");
        recorder
            .record(
                "kan",
                "comment",
                Some(TimelineEntityRef {
                    kind: TimelineEntityKind::Ticket,
                    id: "kan-t10".to_owned(),
                }),
                json!({ "text": "elsewhere" }),
            )
            .expect("second event lands");

        let handler = TimelineQueryHandler::new(store);
        let response = handler
            .handle(&json!({
                "project_id": "kan",
                "entity": entity,
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
                    "project_id": "kan",
                    "kind": "transition",
                    "entity": entity,
                    "recorded_at": "2026-09-04T12:00:01Z",
                    "detail": { "to": "in_progress" },
                }]
            })
        );
    }

    #[test]
    fn timeline_query_rejects_unknown_fields() {
        let store = std::sync::Arc::new(MemoryTimelineStore::default());
        let handler = TimelineQueryHandler::new(store);

        let error = handler
            .handle(&json!({ "project_id": "kan", "surprise": true }))
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
                &json!({ "project_id": "kan", "kinds": ["transition"] }),
            )
            .expect("the core serves timeline queries");

        assert_eq!(response, json!({ "events": [] }));
    }
}
