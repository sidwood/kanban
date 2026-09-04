//! The SQLite implementation of the ruling storage port.

use kanban_app::{RulingStore, TimelineAppend};
use kanban_domain::{Ruling, RulingDraft, RulingEntityRef, RulingId, RulingSummary};
use kanban_dto::{ApiError, RulingListQuery};
use rusqlite::params;
use serde_json::Value;

use crate::db::{ConnectionHandle, Database, WriteSpan};
use crate::timeline::{TimelineAppend as StorageTimelineAppend, insert_event};

/// The ruling port over the authoritative database.
pub struct SqliteRulingStore {
    conn: ConnectionHandle,
}

impl SqliteRulingStore {
    /// Share the connection the `database` owns.
    pub fn new(database: &Database) -> Self {
        Self {
            conn: database.connection_handle(),
        }
    }

    fn lock(&self) -> parking_lot::ReentrantMutexGuard<'_, rusqlite::Connection> {
        self.conn.lock()
    }
}

impl RulingStore for SqliteRulingStore {
    fn insert(&self, draft: &RulingDraft, append: TimelineAppend) -> Result<Ruling, ApiError> {
        let conn = self.lock();
        let span = WriteSpan::begin(&conn).map_err(internal)?;
        let (entity_kind, entity_id) = entity_parts(&draft.entity);
        span.execute(
            "INSERT INTO rulings (project_id, entity_kind, entity_id, summary, supersedes_id)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                draft.project_id,
                entity_kind,
                entity_id,
                draft.summary.as_str(),
                draft.supersedes.map(|id| id.value() as i64),
            ],
        )
        .map_err(internal)?;
        let id = RulingId::new(
            span.last_insert_rowid()
                .try_into()
                .map_err(|_| ApiError::internal("the ruling identity overflowed"))?,
        );
        let ruling = Ruling::restore(
            id,
            draft.project_id.clone(),
            draft.entity.clone(),
            draft.summary.clone(),
            draft.supersedes,
            recorded_at(&span, id)?,
        );
        append_timeline(&span, &ruling, &append)?;
        span.commit().map_err(internal)?;
        Ok(ruling)
    }

    fn find(&self, project_id: &str, id: RulingId) -> Result<Option<Ruling>, ApiError> {
        let conn = self.lock();
        let row = conn.query_row(
            "SELECT id, project_id, entity_kind, entity_id, summary, supersedes_id, recorded_at
             FROM rulings
             WHERE id = ?1 AND project_id = ?2",
            params![id.value() as i64, project_id],
            decode_row,
        );
        match row {
            Ok(ruling) => Ok(Some(ruling)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(error) => Err(ApiError::internal(&error.to_string())),
        }
    }

    fn list(&self, query: &RulingListQuery) -> Result<Vec<Ruling>, ApiError> {
        let conn = self.lock();
        let mut sql = String::from(
            "SELECT id, project_id, entity_kind, entity_id, summary, supersedes_id, recorded_at
             FROM rulings
             WHERE project_id = ?1",
        );
        let mut bindings: Vec<String> = vec![query.project_id.clone()];
        if let Some(entity) = &query.entity {
            sql.push_str(" AND entity_kind = ? AND entity_id = ?");
            bindings.push(entity.kind.as_str().to_owned());
            bindings.push(entity.id.clone());
        }
        sql.push_str(" ORDER BY recorded_at ASC, id ASC");

        let mut statement = conn
            .prepare(&sql)
            .map_err(|error| ApiError::internal(&error.to_string()))?;
        let rows = statement
            .query_map(rusqlite::params_from_iter(bindings.iter()), decode_row)
            .map_err(|error| ApiError::internal(&error.to_string()))?;
        let mut rulings = Vec::new();
        for row in rows {
            rulings.push(row.map_err(|error| ApiError::internal(&error.to_string()))?);
        }
        Ok(rulings)
    }
}

fn entity_parts(entity: &Option<RulingEntityRef>) -> (Option<String>, Option<String>) {
    entity.as_ref().map_or((None, None), |value| {
        (Some(value.kind.clone()), Some(value.id.clone()))
    })
}

fn decode_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Ruling> {
    let id = row.get::<_, i64>(0)?.unsigned_abs();
    let project_id: String = row.get(1)?;
    let entity_kind: Option<String> = row.get(2)?;
    let entity_id: Option<String> = row.get(3)?;
    let summary: String = row.get(4)?;
    let supersedes: Option<i64> = row.get(5)?;
    let recorded_at: String = row.get(6)?;
    let entity = match (entity_kind, entity_id) {
        (Some(kind), Some(id)) => Some(RulingEntityRef { kind, id }),
        (None, None) => None,
        _ => {
            return Err(rusqlite::Error::InvalidColumnType(
                2,
                "entity".to_owned(),
                rusqlite::types::Type::Text,
            ));
        }
    };
    let summary = RulingSummary::new(&summary)
        .map_err(|_| rusqlite::Error::ToSqlConversionFailure(Box::new(CorruptSummary)))?;
    Ok(Ruling::restore(
        RulingId::new(id),
        project_id,
        entity,
        summary,
        supersedes.map(|value| RulingId::new(value.unsigned_abs())),
        recorded_at,
    ))
}

fn recorded_at(conn: &rusqlite::Connection, id: RulingId) -> Result<String, ApiError> {
    conn.query_row(
        "SELECT recorded_at FROM rulings WHERE id = ?1",
        params![id.value() as i64],
        |row| row.get(0),
    )
    .map_err(internal)
}

/// Report a SQLite failure the caller cannot act on.
fn internal(error: rusqlite::Error) -> ApiError {
    ApiError::internal(&error.to_string())
}

#[derive(Debug)]
struct CorruptSummary;

impl std::fmt::Display for CorruptSummary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "a stored ruling summary failed validation")
    }
}

impl std::error::Error for CorruptSummary {}

fn append_timeline(
    conn: &rusqlite::Connection,
    ruling: &Ruling,
    append: &TimelineAppend,
) -> Result<(), ApiError> {
    let mut detail = append.facts.clone();
    let object = detail
        .as_object_mut()
        .ok_or_else(|| ApiError::internal("timeline facts must be a JSON object"))?;
    object.insert("id".to_owned(), Value::from(ruling.id().value()));
    if let Some(supersedes) = ruling.supersedes() {
        object.insert("supersedes_id".to_owned(), Value::from(supersedes.value()));
    }
    let (entity_kind, entity_id) = entity_parts(&ruling.entity().cloned());
    insert_event(
        conn,
        &StorageTimelineAppend {
            project_id: ruling.project_id().to_owned(),
            kind: append.kind.to_owned(),
            entity_kind,
            entity_id,
            detail,
        },
    )
    .map_err(|error| ApiError::internal(&error.to_string()))
}

#[cfg(test)]
mod tests {
    use kanban_app::{RulingStore, TimelineAppend};
    use kanban_domain::{RulingEntityRef, RulingId, RulingSummary};
    use kanban_dto::RulingListQuery;
    use serde_json::json;

    use super::SqliteRulingStore;
    use crate::db::Database;
    use crate::migrations::AllowAllMigrations;
    use crate::test_support::scratch_database;

    fn store() -> (tempfile::TempDir, Database, SqliteRulingStore) {
        let (dir, mut database) = scratch_database();
        database
            .migrate(&AllowAllMigrations)
            .expect("the migrations apply");
        let store = SqliteRulingStore::new(&database);
        (dir, database, store)
    }

    fn append(kind: &'static str, facts: serde_json::Value) -> TimelineAppend {
        TimelineAppend { kind, facts }
    }

    fn draft(
        project_id: &str,
        summary: &str,
        entity: Option<RulingEntityRef>,
        supersedes: Option<RulingId>,
    ) -> kanban_domain::RulingDraft {
        kanban_domain::RulingDraft {
            project_id: project_id.to_owned(),
            entity,
            summary: RulingSummary::new(summary).expect("summary validates"),
            supersedes,
        }
    }

    #[test]
    fn inserting_lands_the_row_and_its_timeline_append() {
        let (_dir, database, store) = store();
        let ruling = store
            .insert(
                &draft(
                    "kan",
                    "Allow landing",
                    Some(RulingEntityRef {
                        kind: "ticket".to_owned(),
                        id: "kan-t12".to_owned(),
                    }),
                    None,
                ),
                append("ruling", json!({ "summary": "Allow landing" })),
            )
            .expect("the insert lands");

        assert_eq!(ruling.id(), RulingId::new(1));
        let conn = database.connection();
        let summary: String = conn
            .query_row("SELECT summary FROM rulings WHERE id = 1", [], |row| {
                row.get(0)
            })
            .expect("the row is readable");
        assert_eq!(summary, "Allow landing");
        let kind: String = conn
            .query_row("SELECT kind FROM timeline_events WHERE id = 1", [], |row| {
                row.get(0)
            })
            .expect("the timeline append lands");
        assert_eq!(kind, "ruling");
    }

    #[test]
    fn updating_rulings_fails() {
        let (_dir, database, store) = store();
        store
            .insert(
                &draft("kan", "Hold", None, None),
                append("ruling", json!({ "summary": "Hold" })),
            )
            .expect("the insert lands");

        let outcome = database
            .connection()
            .execute("UPDATE rulings SET summary = 'tampered'", []);

        let error = outcome.expect_err("the schema must refuse updates");
        assert!(
            error.to_string().contains("append-only"),
            "the refusal should say append-only, got: {error}"
        );
    }

    #[test]
    fn deleting_rulings_fails() {
        let (_dir, database, store) = store();
        store
            .insert(
                &draft("kan", "Hold", None, None),
                append("ruling", json!({ "summary": "Hold" })),
            )
            .expect("the insert lands");

        let outcome = database.connection().execute("DELETE FROM rulings", []);

        let error = outcome.expect_err("the schema must refuse deletes");
        assert!(
            error.to_string().contains("append-only"),
            "the refusal should say append-only, got: {error}"
        );
    }

    #[test]
    fn superseding_keeps_both_records_visible() {
        let (_dir, _database, store) = store();
        let original = store
            .insert(
                &draft("kan", "Hold", None, None),
                append("ruling", json!({ "summary": "Hold" })),
            )
            .expect("the original lands");
        store
            .insert(
                &draft("kan", "Proceed", None, Some(original.id())),
                append(
                    "ruling",
                    json!({ "summary": "Proceed", "supersedes_id": original.id().value() }),
                ),
            )
            .expect("the superseding record lands");

        let listed = store
            .list(&RulingListQuery {
                project_id: "kan".to_owned(),
                entity: None,
            })
            .expect("the list serves");

        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0].summary().as_str(), "Hold");
        assert!(listed[0].supersedes().is_none());
        assert_eq!(listed[1].summary().as_str(), "Proceed");
        assert_eq!(listed[1].supersedes(), Some(original.id()));
    }
}
