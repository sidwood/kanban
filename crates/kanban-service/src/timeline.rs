//! SQLite-backed implementation of the timeline query port.
//!
//! Rows are written by the entity stores inside their own
//! transactions; this adapter reads them back and refuses any row
//! that no longer decodes into the payload vocabulary.

use std::sync::Arc;

use kanban_app::{TimelineError, TimelineStore};
use kanban_dto::{
    TimelineEntityKind, TimelineEntityRef, TimelineEventKind, TimelineEventRecord, TimelineQuery,
    TimelineScope,
};
use kanban_storage::{Database, TimelineFilter, TimelineRow};

/// The storage adapter the core uses for timeline queries.
///
/// It shares the database handle rather than wrapping it: the
/// database already serialises its own connection, and a second lock
/// here would duplicate ownership of the authoritative connection.
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
    fn query(&self, query: &TimelineQuery) -> Result<Vec<TimelineEventRecord>, TimelineError> {
        let filter = TimelineFilter {
            scope: query.scope.clone(),
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
    let scope = row_scope(&row.scope, row.project_id)?;
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
        scope,
        kind,
        entity,
        recorded_at: row.recorded_at,
        detail: row.detail,
    })
}

fn row_scope(scope: &str, project_id: String) -> Result<TimelineScope, TimelineError> {
    match scope {
        "global" if project_id.is_empty() => Ok(TimelineScope::Global),
        "global" => Err(TimelineError::Storage(format!(
            "a global timeline row named the Project `{project_id}`"
        ))),
        "project" if project_id.is_empty() => Err(TimelineError::Storage(
            "a Project timeline row named no Project".to_owned(),
        )),
        "project" => Ok(TimelineScope::Project(project_id)),
        other => Err(TimelineError::Storage(format!(
            "unknown stored timeline scope `{other}`"
        ))),
    }
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
