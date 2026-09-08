//! Append-only result records share the Core's SQLite transaction.
use crate::db::{ConnectionHandle, Database, WriteSpan};
use crate::timeline::insert_event;
use kanban_app::TimelineEnvelope;
use kanban_app::submission::{SubmissionContext, SubmissionStore};
use kanban_dto::{ApiError, SubmissionRecord};
use rusqlite::{OptionalExtension, params};

pub struct SqliteSubmissionStore {
    conn: ConnectionHandle,
}
impl SqliteSubmissionStore {
    pub fn new(database: &Database) -> Self {
        Self {
            conn: database.connection_handle(),
        }
    }
}
impl SubmissionStore for SqliteSubmissionStore {
    fn list(&self, project_id: u64) -> Result<Vec<SubmissionRecord>, ApiError> {
        let conn = self.conn.lock();
        let mut statement = conn
            .prepare("SELECT id, record FROM submissions WHERE project_id = ?1 ORDER BY id")
            .map_err(internal)?;
        let rows = statement
            .query_map([project_id as i64], |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
            })
            .map_err(internal)?;
        rows.map(|row| {
            let (id, body) = row.map_err(internal)?;
            let mut record: SubmissionRecord = serde_json::from_str(&body).map_err(internal)?;
            record.id = id as u64;
            Ok(record)
        })
        .collect()
    }
    fn context(&self, run: u64) -> Result<SubmissionContext, ApiError> {
        self.conn.lock().query_row(
            "SELECT project_id, ticket_id, dispatch_request_id, version FROM runs WHERE id = ?1",
            [run as i64], |r| Ok(SubmissionContext { project_id: r.get::<_, i64>(0)? as u64,
                ticket_id: r.get::<_, i64>(1)? as u64, dispatch_request_id: r.get::<_, i64>(2)? as u64,
                version: r.get::<_, i64>(3)? as u64 }))
            .optional().map_err(internal)?.ok_or_else(|| ApiError::not_found("run"))
    }
    fn append(
        &self,
        mut record: SubmissionRecord,
        envelope: &dyn Fn(&SubmissionRecord) -> TimelineEnvelope,
        reviewed: &kanban_app::review_execution::ReviewChangeObserver<'_>,
    ) -> Result<SubmissionRecord, ApiError> {
        let conn = self.conn.lock();
        let span = WriteSpan::begin(&conn).map_err(internal)?;
        let exists: bool = span
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM submissions WHERE run_id = ?1)",
                [record.run_id as i64],
                |r| r.get(0),
            )
            .map_err(internal)?;
        if exists {
            return Err(ApiError::invalid_request(
                "this run already has an immutable submission",
            ));
        }
        // The write span serialises identity allocation with the immutable row.
        record.id = span
            .query_row(
                "SELECT COALESCE(MAX(id), 0) + 1 FROM submissions",
                [],
                |row| row.get::<_, i64>(0),
            )
            .map_err(internal)? as u64;
        span.execute(
            "INSERT INTO submissions (project_id, ticket_id, run_id, capability_id, record, id)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                record.project_id as i64,
                record.ticket_id as i64,
                record.run_id as i64,
                record.capability_id as i64,
                serde_json::to_string(&record).map_err(internal)?,
                record.id as i64,
            ],
        )
        .map_err(internal)?;
        crate::review_execution::accept_submission(&span, &record, reviewed)?;
        insert_event(&span, &envelope(&record)).map_err(internal)?;
        span.commit().map_err(internal)?;
        Ok(record)
    }
}
fn internal(error: impl std::fmt::Display) -> ApiError {
    ApiError::internal(&error.to_string())
}
