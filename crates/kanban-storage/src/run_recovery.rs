//! Recovery history and its immutable Ruling share the application's write span.
use crate::SqliteRulingStore;
use crate::db::{ConnectionHandle, Database, WriteSpan};
use kanban_app::run_recovery::PendingRunResume;
use kanban_app::{RulingStore, RunRecoveryContext, RunRecoveryStore, TimelineFacts};
use kanban_domain::{ProjectId, Ruling, RulingEntityRef, RulingSummary, RunId, TicketId};
use kanban_dto::{ApiError, RunRecoveryAction, RunRecoveryRecord, TimelineEventKind};
use rusqlite::{OptionalExtension, params};
use serde_json::json;

pub struct SqliteRunRecoveryStore {
    conn: ConnectionHandle,
    rulings: SqliteRulingStore,
}

impl SqliteRunRecoveryStore {
    pub fn new(database: &Database) -> Self {
        Self {
            conn: database.connection_handle(),
            rulings: SqliteRulingStore::new(database),
        }
    }
    fn record(
        &self,
        run: RunId,
        summary: RulingSummary,
        action: RunRecoveryAction,
        replacement: Option<u64>,
    ) -> Result<RunRecoveryRecord, ApiError> {
        let conn = self.conn.lock();
        let span = WriteSpan::begin(&conn).map_err(internal)?;
        let context = self
            .context(run)?
            .ok_or_else(|| ApiError::not_found("run"))?;
        if action == RunRecoveryAction::Resume && (!context.can_resume() || context.pending_resume)
        {
            return Err(ApiError::invalid_request(
                "this run cannot accept another resume intent",
            ));
        }
        let draft = Ruling::record(
            context.project.value(),
            summary,
            Some(RulingEntityRef {
                kind: "run".to_owned(),
                id: run.value().to_string(),
            }),
        );
        let ruling = self.rulings.insert(&draft, TimelineFacts {
            kind: TimelineEventKind::Ruling,
            facts: json!({"action":action,"replacement_dispatch_request_id":replacement,"run_id":run.value(),"summary":draft.summary.as_str()}),
        })?;
        span.execute(
            "INSERT INTO run_recoveries (run_id, ruling_id, sequence, action, replacement_dispatch_request_id) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                integer(run.value())?,
                integer(ruling.id().value())?,
                integer(context.version + 1)?,
                match action { RunRecoveryAction::OperatorRuling => "operator_ruling", RunRecoveryAction::Retry => "retry", RunRecoveryAction::Resume => "resume" },
                replacement.map(integer).transpose()?
            ],
        )
        .map_err(internal)?;
        let id = span.last_insert_rowid();
        if action == RunRecoveryAction::Resume {
            span.execute(
                "INSERT INTO run_resume_deliveries(recovery_id) VALUES (?1)",
                [id],
            )
            .map_err(internal)?;
        }
        let record = span
            .query_row(
                "SELECT recovery.id, recovery.run_id, runs.project_id, rulings.summary,
                    recovery.ruling_id, recovery.created_at, recovery.sequence, recovery.action, recovery.replacement_dispatch_request_id
             FROM run_recoveries recovery JOIN runs ON runs.id=recovery.run_id
             JOIN rulings ON rulings.id=recovery.ruling_id WHERE recovery.id=?1",
                [id],
                decode,
            )
            .map_err(internal)?;
        span.commit().map_err(internal)?;
        Ok(record)
    }
}

fn integer(id: u64) -> Result<i64, ApiError> {
    id.try_into()
        .map_err(|_| ApiError::invalid_request("the run identity is out of range"))
}
fn internal(error: rusqlite::Error) -> ApiError {
    ApiError::internal(&error.to_string())
}

impl RunRecoveryStore for SqliteRunRecoveryStore {
    fn context(&self, run: RunId) -> Result<Option<RunRecoveryContext>, ApiError> {
        let conn = self.conn.lock();
        let mut context = conn.query_row(
            "SELECT project_id, ticket_id, (SELECT count(*) FROM run_recoveries WHERE run_id=runs.id),
               EXISTS(SELECT 1 FROM run_recoveries WHERE run_id=runs.id AND action='retry'),
               EXISTS(SELECT 1 FROM submissions WHERE run_id=runs.id) OR EXISTS(
                 SELECT 1 FROM review_slot_verdicts v JOIN dispatch_requests d ON d.reviewer_slot_id=v.slot_id WHERE d.id=runs.dispatch_request_id),
               dispatch_request_id, EXISTS(SELECT 1 FROM capabilities c WHERE c.dispatch_request_id=runs.dispatch_request_id AND c.status='active' AND c.settled_at IS NULL),
               EXISTS(SELECT 1 FROM run_resume_deliveries d JOIN run_recoveries r ON r.id=d.recovery_id WHERE r.run_id=runs.id AND d.status='pending')
             FROM runs WHERE id=?1", [integer(run.value())?], |row| Ok(RunRecoveryContext {
                project: ProjectId::new(row.get::<_, i64>(0)? as u64),
                ticket: TicketId::new(row.get::<_, i64>(1)? as u64),
                version: row.get::<_, i64>(2)? as u64,
                superseded: row.get(3)?,
                has_submission: row.get(4)?,
                dispatch_request_id: row.get::<_,i64>(5)? as u64,
                resumable: row.get(6)?,
                review_eligible: false,
                pending_resume: row.get(7)?,
            }),
        ).optional().map_err(internal)?;
        if let Some(context) = &mut context {
            context.review_eligible =
                crate::review_execution::request_is_active(&conn, context.dispatch_request_id)?;
        }
        Ok(context)
    }

    fn rule(&self, run: RunId, summary: RulingSummary) -> Result<RunRecoveryRecord, ApiError> {
        self.record(run, summary, RunRecoveryAction::OperatorRuling, None)
    }

    fn resume(&self, run: RunId, summary: RulingSummary) -> Result<RunRecoveryRecord, ApiError> {
        self.record(run, summary, RunRecoveryAction::Resume, None)
    }

    fn retry(&self, run: RunId, summary: RulingSummary) -> Result<RunRecoveryRecord, ApiError> {
        let conn = self.conn.lock();
        let span = WriteSpan::begin(&conn).map_err(internal)?;
        let old_request: i64 = span
            .query_row(
                "SELECT dispatch_request_id FROM runs WHERE id=?1",
                [integer(run.value())?],
                |row| row.get(0),
            )
            .map_err(internal)?;
        crate::review_execution::guard_active_request(&span, old_request as u64)?;
        let changed = span.execute("UPDATE dispatch_requests SET completed_at=unixepoch(), version=version+1 WHERE id=?1 AND completed_at IS NULL",
            [old_request]).map_err(internal)?;
        if changed != 1 {
            return Err(ApiError::invalid_request(
                "the original attempt is no longer open",
            ));
        }
        span.execute("UPDATE capabilities SET status='settled', settled_at=coalesce(settled_at,unixepoch()) WHERE dispatch_request_id=?1",
            [old_request]).map_err(internal)?;
        span.execute("INSERT INTO dispatch_requests(project_id,ticket_id,status,priority,ready,harness,model,usage_pool,created_at,version,reviewer_slot_id)
            SELECT project_id,ticket_id,'queued',priority,ready,harness,model,usage_pool,unixepoch(),1,reviewer_slot_id FROM dispatch_requests WHERE id=?1",
            [old_request]).map_err(internal)?;
        let replacement = span.last_insert_rowid();
        span.execute(
            "UPDATE review_slots SET dispatch_request_id=?2 WHERE dispatch_request_id=?1",
            params![old_request, replacement],
        )
        .map_err(internal)?;
        let record = self.record(
            run,
            summary,
            RunRecoveryAction::Retry,
            Some(replacement as u64),
        )?;
        span.commit().map_err(internal)?;
        Ok(record)
    }

    fn pending_resume_ids(&self, project: ProjectId) -> Result<Vec<u64>, ApiError> {
        let conn = self.conn.lock();
        let mut statement = conn
            .prepare(
                "SELECT d.recovery_id FROM run_resume_deliveries d
            JOIN run_recoveries r ON r.id=d.recovery_id JOIN runs ON runs.id=r.run_id
            WHERE runs.project_id=?1 AND d.status='pending' AND d.next_attempt_at<=unixepoch()
            ORDER BY d.next_attempt_at,d.recovery_id LIMIT 32",
            )
            .map_err(internal)?;
        statement
            .query_map([integer(project.value())?], |r| {
                Ok(r.get::<_, i64>(0)? as u64)
            })
            .map_err(internal)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(internal)
    }

    fn prepare_resume_delivery(&self, id: u64) -> Result<Option<PendingRunResume>, ApiError> {
        let conn = self.conn.lock();
        let span = WriteSpan::begin(&conn).map_err(internal)?;
        let run = span.query_row("SELECT r.run_id FROM run_resume_deliveries d JOIN run_recoveries r ON r.id=d.recovery_id
            WHERE d.recovery_id=?1 AND d.status='pending' AND d.next_attempt_at<=unixepoch()",
            [integer(id)?], |r| Ok(r.get::<_,i64>(0)? as u64)).optional().map_err(internal)?;
        let Some(run) = run else {
            return Ok(None);
        };
        let context = self
            .context(RunId::new(run))?
            .ok_or_else(|| ApiError::not_found("run"))?;
        if !context.can_resume() {
            span.execute(
                "UPDATE run_resume_deliveries SET status='obsolete' WHERE recovery_id=?1",
                [integer(id)?],
            )
            .map_err(internal)?;
            span.commit().map_err(internal)?;
            return Ok(None);
        }
        span.execute("UPDATE run_resume_deliveries SET attempts=attempts+1,next_attempt_at=unixepoch()+1 WHERE recovery_id=?1", [integer(id)?]).map_err(internal)?;
        span.commit().map_err(internal)?;
        Ok(Some(PendingRunResume {
            recovery_id: id,
            run_id: run,
            dispatch_request_id: context.dispatch_request_id,
        }))
    }

    fn acknowledge_resume_delivery(&self, id: u64) -> Result<(), ApiError> {
        self.conn.lock().execute("UPDATE run_resume_deliveries SET status='delivered' WHERE recovery_id=?1 AND status='pending'",[integer(id)?]).map_err(internal)?;
        Ok(())
    }

    fn list(&self, run: RunId) -> Result<Vec<RunRecoveryRecord>, ApiError> {
        let conn = self.conn.lock();
        let mut statement = conn
            .prepare(
                "SELECT recovery.id, recovery.run_id, runs.project_id, rulings.summary,
                    recovery.ruling_id, recovery.created_at, recovery.sequence, recovery.action, recovery.replacement_dispatch_request_id
             FROM run_recoveries recovery JOIN runs ON runs.id=recovery.run_id
             JOIN rulings ON rulings.id=recovery.ruling_id
             WHERE recovery.run_id=?1 ORDER BY recovery.sequence",
            )
            .map_err(internal)?;
        statement
            .query_map([integer(run.value())?], decode)
            .map_err(internal)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(internal)
    }
}

fn decode(row: &rusqlite::Row<'_>) -> rusqlite::Result<RunRecoveryRecord> {
    Ok(RunRecoveryRecord {
        id: row.get::<_, i64>(0)? as u64,
        run_id: row.get::<_, i64>(1)? as u64,
        project_id: row.get::<_, i64>(2)? as u64,
        action: match row.get::<_, String>(7)?.as_str() {
            "operator_ruling" => RunRecoveryAction::OperatorRuling,
            "retry" => RunRecoveryAction::Retry,
            "resume" => RunRecoveryAction::Resume,
            _ => {
                return Err(rusqlite::Error::InvalidColumnType(
                    7,
                    "action".into(),
                    rusqlite::types::Type::Text,
                ));
            }
        },
        replacement_dispatch_request_id: row.get::<_, Option<i64>>(8)?.map(|id| id as u64),
        summary: row.get(3)?,
        ruling_id: row.get::<_, i64>(4)? as u64,
        created_at: row.get::<_, i64>(5)? as u64,
        version: row.get::<_, i64>(6)? as u64,
    })
}
