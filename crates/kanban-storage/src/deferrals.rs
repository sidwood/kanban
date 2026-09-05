//! The SQLite implementation of the deferral storage port.

use std::fmt;

use kanban_app::{
    DeferralStore, TimelineEnvelope, TimelineFacts, already_superseded_deferral_error,
};
use kanban_domain::{Deferral, DeferralDraft, DeferralId, DeferralReason};
use kanban_dto::{ApiError, DeferralListQuery, TimelineEntityKind, TimelineEntityRef};
use rusqlite::params;
use serde_json::Value;

use crate::db::{ConnectionHandle, Database, WriteSpan};
use crate::timeline::insert_event;

/// The deferral port over the authoritative database.
pub struct SqliteDeferralStore {
    conn: ConnectionHandle,
}

impl SqliteDeferralStore {
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

impl DeferralStore for SqliteDeferralStore {
    fn insert(&self, draft: &DeferralDraft, facts: TimelineFacts) -> Result<Deferral, ApiError> {
        let conn = self.lock();
        let span = WriteSpan::begin(&conn).map_err(internal)?;
        if let Some(supersedes) = draft.supersedes {
            if has_successor(&span, draft.project_id, supersedes)? {
                return Err(already_superseded_deferral_error(supersedes.value()));
            }
        }
        span.execute(
            "INSERT INTO deferrals (project_id, finding_id, reason, supersedes_id)
                 VALUES (?1, ?2, ?3, ?4)",
            params![
                draft.project_id.to_string(),
                draft.finding_id,
                draft.reason.as_str(),
                draft.supersedes.map(|id| id.value() as i64),
            ],
        )
        .map_err(internal)?;
        let id = DeferralId::new(
            span.last_insert_rowid()
                .try_into()
                .map_err(|_| ApiError::internal("the deferral identity overflowed"))?,
        );
        let deferral = Deferral::restore(
            id,
            draft.project_id,
            draft.finding_id.clone(),
            draft.reason.clone(),
            draft.supersedes,
            recorded_at(&span, id)?,
        );
        append_timeline(&span, &deferral, &facts)?;
        span.commit().map_err(internal)?;
        Ok(deferral)
    }

    fn find(&self, project_id: u64, id: DeferralId) -> Result<Option<Deferral>, ApiError> {
        let conn = self.lock();
        let row = conn.query_row(
            "SELECT id, project_id, finding_id, reason, supersedes_id, recorded_at
             FROM deferrals
             WHERE id = ?1 AND project_id = ?2",
            params![id.value() as i64, project_id.to_string()],
            decode_row,
        );
        match row {
            Ok(deferral) => Ok(Some(deferral)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(error) => Err(ApiError::internal(&error.to_string())),
        }
    }

    fn has_successor(&self, project_id: u64, id: DeferralId) -> Result<bool, ApiError> {
        let conn = self.lock();
        has_successor(&conn, project_id, id)
    }

    fn list(&self, query: &DeferralListQuery) -> Result<Vec<Deferral>, ApiError> {
        let conn = self.lock();
        let mut sql = String::from(
            "SELECT id, project_id, finding_id, reason, supersedes_id, recorded_at
             FROM deferrals
             WHERE project_id = ?1",
        );
        let mut bindings: Vec<String> = vec![query.project_id.to_string()];
        if let Some(finding_id) = &query.finding_id {
            sql.push_str(" AND finding_id = ?");
            bindings.push(finding_id.clone());
        }
        sql.push_str(" ORDER BY recorded_at ASC, id ASC");

        let mut statement = conn
            .prepare(&sql)
            .map_err(|error| ApiError::internal(&error.to_string()))?;
        let rows = statement
            .query_map(rusqlite::params_from_iter(bindings.iter()), decode_row)
            .map_err(|error| ApiError::internal(&error.to_string()))?;
        let mut deferrals = Vec::new();
        for row in rows {
            deferrals.push(row.map_err(|error| ApiError::internal(&error.to_string()))?);
        }
        Ok(deferrals)
    }
}

fn decode_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Deferral> {
    let id = row.get::<_, i64>(0)?.unsigned_abs();
    // A non-numeric scope is a legacy row migration 0017 missed;
    // refusing it beats guessing which Project owns it.
    let project_id: u64 = row.get::<_, String>(1)?.parse().map_err(|_| {
        rusqlite::Error::FromSqlConversionFailure(
            1,
            rusqlite::types::Type::Text,
            Box::new(CorruptScope),
        )
    })?;
    let finding_id: String = row.get(2)?;
    let reason: String = row.get(3)?;
    let supersedes: Option<i64> = row.get(4)?;
    let recorded_at: String = row.get(5)?;
    let reason = DeferralReason::new(&reason)
        .map_err(|_| rusqlite::Error::ToSqlConversionFailure(Box::new(CorruptReason)))?;
    Ok(Deferral::restore(
        DeferralId::new(id),
        project_id,
        finding_id,
        reason,
        supersedes.map(|value| DeferralId::new(value.unsigned_abs())),
        recorded_at,
    ))
}

fn recorded_at(conn: &rusqlite::Connection, id: DeferralId) -> Result<String, ApiError> {
    conn.query_row(
        "SELECT recorded_at FROM deferrals WHERE id = ?1",
        params![id.value() as i64],
        |row| row.get(0),
    )
    .map_err(internal)
}

fn has_successor(
    conn: &rusqlite::Connection,
    project_id: u64,
    id: DeferralId,
) -> Result<bool, ApiError> {
    match conn.query_row(
        "SELECT 1 FROM deferrals
         WHERE project_id = ?1 AND supersedes_id = ?2
         LIMIT 1",
        params![project_id.to_string(), id.value() as i64],
        |row| row.get::<_, i64>(0),
    ) {
        Ok(_) => Ok(true),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(false),
        Err(error) => Err(internal(error)),
    }
}

/// Report a SQLite failure the caller cannot act on.
fn internal(error: rusqlite::Error) -> ApiError {
    ApiError::internal(&error.to_string())
}

#[derive(Debug)]
struct CorruptReason;

/// A stored Project scope was not a numeric identity.
#[derive(Debug)]
struct CorruptScope;

impl std::fmt::Display for CorruptScope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "a stored deferral names a non-numeric Project scope")
    }
}

impl std::error::Error for CorruptScope {}

impl std::fmt::Display for CorruptReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "a stored deferral reason failed validation")
    }
}

impl std::error::Error for CorruptReason {}

fn append_timeline(
    conn: &rusqlite::Connection,
    deferral: &Deferral,
    facts: &TimelineFacts,
) -> Result<(), ApiError> {
    let mut detail = facts.facts.clone();
    let object = detail
        .as_object_mut()
        .ok_or_else(|| ApiError::internal("timeline facts must be a JSON object"))?;
    object.insert("id".to_owned(), Value::from(deferral.id().value()));
    object.insert(
        "finding_id".to_owned(),
        Value::from(deferral.finding_id().to_owned()),
    );
    if let Some(supersedes) = deferral.supersedes() {
        object.insert("supersedes_id".to_owned(), Value::from(supersedes.value()));
    }
    let envelope = TimelineEnvelope::project(
        deferral.project_id(),
        facts.kind,
        Some(TimelineEntityRef {
            kind: TimelineEntityKind::Finding,
            id: deferral.finding_id().to_owned(),
        }),
        detail,
    );
    insert_event(conn, &envelope).map_err(|error| ApiError::internal(&error.to_string()))
}

#[cfg(test)]
mod tests {
    use kanban_app::{DeferralStore, TimelineFacts};
    use kanban_domain::{DeferralId, DeferralReason};
    use kanban_dto::DeferralListQuery;
    use serde_json::json;

    use super::SqliteDeferralStore;
    use crate::db::Database;
    use crate::migrations::AllowAllMigrations;
    use crate::test_support::scratch_database;

    fn store() -> (tempfile::TempDir, Database, SqliteDeferralStore) {
        let (dir, mut database) = scratch_database();
        database
            .migrate(&AllowAllMigrations)
            .expect("the migrations apply");
        let store = SqliteDeferralStore::new(&database);
        (dir, database, store)
    }

    fn append(facts: serde_json::Value) -> TimelineFacts {
        TimelineFacts {
            kind: kanban_dto::TimelineEventKind::Deferral,
            facts,
        }
    }

    fn draft(
        project_id: u64,
        finding_id: &str,
        reason: &str,
        supersedes: Option<DeferralId>,
    ) -> kanban_domain::DeferralDraft {
        kanban_domain::DeferralDraft {
            project_id,
            finding_id: finding_id.to_owned(),
            reason: DeferralReason::new(reason).expect("reason validates"),
            supersedes,
        }
    }

    #[test]
    fn inserting_lands_the_row_and_its_timeline_append() {
        let (_dir, database, store) = store();
        let deferral = store
            .insert(
                &draft(1, "finding-1", "Cosmetic only", None),
                append(json!({ "reason": "Cosmetic only" })),
            )
            .expect("the insert lands");

        assert_eq!(deferral.id(), DeferralId::new(1));
        let conn = database.connection();
        let reason: String = conn
            .query_row("SELECT reason FROM deferrals WHERE id = 1", [], |row| {
                row.get(0)
            })
            .expect("the row is readable");
        assert_eq!(reason, "Cosmetic only");
    }

    #[test]
    fn updating_deferrals_fails() {
        let (_dir, database, store) = store();
        store
            .insert(
                &draft(1, "finding-1", "Cosmetic only", None),
                append(json!({ "reason": "Cosmetic only" })),
            )
            .expect("the insert lands");

        let outcome = database
            .connection()
            .execute("UPDATE deferrals SET reason = 'tampered'", []);

        let error = outcome.expect_err("the schema must refuse updates");
        assert!(
            error.to_string().contains("append-only"),
            "the refusal should say append-only, got: {error}"
        );
    }

    #[test]
    fn deleting_deferrals_fails() {
        let (_dir, database, store) = store();
        store
            .insert(
                &draft(1, "finding-1", "Cosmetic only", None),
                append(json!({ "reason": "Cosmetic only" })),
            )
            .expect("the insert lands");

        let outcome = database.connection().execute("DELETE FROM deferrals", []);

        let error = outcome.expect_err("the schema must refuse deletes");
        assert!(
            error.to_string().contains("append-only"),
            "the refusal should say append-only, got: {error}"
        );
    }

    #[test]
    fn inserting_a_second_successor_for_the_same_deferral_fails() {
        let (_dir, _database, store) = store();
        let original = store
            .insert(
                &draft(1, "finding-1", "Cosmetic only", None),
                append(json!({ "reason": "Cosmetic only" })),
            )
            .expect("the original lands");
        store
            .insert(
                &draft(1, "finding-1", "Accepted risk", Some(original.id())),
                append(json!({
                    "reason": "Accepted risk",
                    "supersedes_id": original.id().value(),
                })),
            )
            .expect("the first successor lands");
        let error = store
            .insert(
                &draft(1, "finding-1", "Reopened", Some(original.id())),
                append(json!({
                    "reason": "Reopened",
                    "supersedes_id": original.id().value(),
                })),
            )
            .expect_err("a second successor is refused");

        assert_eq!(error.code, kanban_dto::ErrorCode::InvalidRequest);
        assert!(
            error.message.contains("already has a successor"),
            "the refusal should name the invariant: {}",
            error.message
        );
    }

    #[test]
    fn superseding_keeps_both_records_visible() {
        let (_dir, _database, store) = store();
        let original = store
            .insert(
                &draft(1, "finding-1", "Cosmetic only", None),
                append(json!({ "reason": "Cosmetic only" })),
            )
            .expect("the original lands");
        store
            .insert(
                &draft(1, "finding-1", "Accepted risk", Some(original.id())),
                append(json!({
                    "reason": "Accepted risk",
                    "supersedes_id": original.id().value(),
                })),
            )
            .expect("the superseding record lands");

        let listed = store
            .list(&DeferralListQuery {
                project_id: 1,
                finding_id: None,
            })
            .expect("the list serves");

        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0].reason().as_str(), "Cosmetic only");
        assert!(listed[0].supersedes().is_none());
        assert_eq!(listed[1].reason().as_str(), "Accepted risk");
        assert_eq!(listed[1].supersedes(), Some(original.id()));
    }

    #[test]
    fn has_successor_propagates_sqlite_errors() {
        let (_dir, database, store) = store();
        let deferral = store
            .insert(
                &draft(1, "finding-1", "Cosmetic only", None),
                append(json!({ "reason": "Cosmetic only" })),
            )
            .expect("the deferral lands");
        database
            .connection()
            .execute("ALTER TABLE deferrals RENAME TO deferrals_hidden", [])
            .expect("the table rename breaks the query");

        let error = store
            .has_successor(1, deferral.id())
            .expect_err("storage errors propagate");

        assert_eq!(error.code, kanban_dto::ErrorCode::Internal);
    }
}
