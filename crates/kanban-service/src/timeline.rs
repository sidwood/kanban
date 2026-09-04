//! SQLite-backed implementation of the timeline port.

use std::sync::Arc;

use kanban_app::{TimelineError, TimelineStore};
use kanban_dto::{
    TimelineEntityKind, TimelineEntityRef, TimelineEventKind, TimelineEventRecord, TimelineQuery,
};
use kanban_storage::{Database, TimelineAppend, TimelineFilter, TimelineRow};
use serde_json::Value;

/// The storage adapter the core uses for timeline queries and
/// appends.
///
/// It shares the database handle rather than wrapping it: the
/// database already serialises its own connection, and a second lock
/// here would let a query hold this one while waiting for the
/// connection a mutation span holds, which is a deadlock as soon as
/// a command appends through this port.
pub struct StorageTimelineStore {
    database: Arc<Database>,
}

impl StorageTimelineStore {
    /// Wraps the authoritative database handle.
    pub fn new(database: Arc<Database>) -> Self {
        Self { database }
    }
}

impl TimelineStore for StorageTimelineStore {
    fn append(
        &self,
        project_id: &str,
        kind: TimelineEventKind,
        entity: Option<TimelineEntityRef>,
        detail: Value,
    ) -> Result<(), TimelineError> {
        let append = TimelineAppend {
            project_id: project_id.to_owned(),
            kind: kind.as_str().to_owned(),
            entity_kind: entity.as_ref().map(|value| value.kind.as_str().to_owned()),
            entity_id: entity.map(|value| value.id),
            detail,
        };
        self.database
            .append_timeline_event(&append)
            .map_err(|error| TimelineError::Storage(error.to_string()))
    }

    fn query(&self, query: &TimelineQuery) -> Result<Vec<TimelineEventRecord>, TimelineError> {
        let filter = TimelineFilter {
            project_id: query.project_id.clone(),
            entity_kind: query
                .entity
                .as_ref()
                .map(|entity| entity.kind.as_str().to_owned()),
            entity_id: query.entity.as_ref().map(|entity| entity.id.clone()),
            kinds: query
                .kinds
                .as_ref()
                .map(|kinds| kinds.iter().map(|kind| kind.as_str().to_owned()).collect())
                .unwrap_or_default(),
            since: query.since.clone(),
            until: query.until.clone(),
        };
        let rows = self
            .database
            .query_timeline(&filter)
            .map_err(|error| TimelineError::Storage(error.to_string()))?;
        rows.into_iter().map(row_to_record).collect()
    }
}

fn row_to_record(row: TimelineRow) -> Result<TimelineEventRecord, TimelineError> {
    let kind = dto_event_kind(&row.kind)?;
    let entity = match (row.entity_kind, row.entity_id) {
        (Some(kind), Some(id)) => Some(TimelineEntityRef {
            kind: dto_entity_kind(&kind)?,
            id,
        }),
        (None, None) => None,
        _ => {
            return Err(TimelineError::Storage(
                "timeline row carried a partial entity reference".to_owned(),
            ));
        }
    };
    Ok(TimelineEventRecord {
        id: row.id,
        project_id: row.project_id,
        kind,
        entity,
        recorded_at: row.recorded_at,
        detail: row.detail,
    })
}

fn dto_event_kind(kind: &str) -> Result<TimelineEventKind, TimelineError> {
    TimelineEventKind::parse(kind).ok_or_else(|| {
        TimelineError::Storage(format!("unknown stored timeline event kind `{kind}`"))
    })
}

fn dto_entity_kind(kind: &str) -> Result<TimelineEntityKind, TimelineError> {
    TimelineEntityKind::parse(kind).ok_or_else(|| {
        TimelineError::Storage(format!("unknown stored timeline entity kind `{kind}`"))
    })
}
