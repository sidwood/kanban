//! Recovery history and its immutable Ruling share the application's write span.
use std::sync::Arc;

use crate::SqliteRulingStore;
use crate::db::{ConnectionHandle, Database, WriteSpan};
use crate::delivery_gate::DeliveryGate;
use kanban_app::run_recovery::{PendingRunResume, ResumeDelivery, ResumeDispatch};
use kanban_app::{RulingStore, RunRecoveryContext, RunRecoveryStore, TimelineFacts, refuse_resume};
use kanban_domain::{
    ProjectId, Ruling, RulingEntityRef, RulingSummary, RunId, TicketId, TicketState,
};
use kanban_dto::{ApiError, RunRecoveryAction, RunRecoveryRecord, TimelineEventKind};
use rusqlite::{OptionalExtension, params};
use serde_json::json;

pub struct SqliteRunRecoveryStore {
    conn: ConnectionHandle,
    delivery_gate: Arc<DeliveryGate>,
    rulings: SqliteRulingStore,
}

impl SqliteRunRecoveryStore {
    pub fn new(database: &Database) -> Self {
        Self {
            conn: database.connection_handle(),
            delivery_gate: database.delivery_gate(),
            rulings: SqliteRulingStore::new(database),
        }
    }

    /// Recheck current custody and reserve one bounded attempt, or
    /// close an intent current custody no longer admits. The caller
    /// already holds the delivery gate, so the custody this reads is
    /// the custody the prompt will be sent under.
    fn authorise(&self, id: u64) -> Result<Result<PendingRunResume, ResumeDelivery>, ApiError> {
        let conn = self.conn.lock();
        let span = WriteSpan::begin(&conn).map_err(internal)?;
        let run = span.query_row("SELECT r.run_id FROM run_resume_deliveries d JOIN run_recoveries r ON r.id=d.recovery_id
            WHERE d.recovery_id=?1 AND d.status='pending' AND d.next_attempt_at<=unixepoch()",
            [integer(id)?], |r| Ok(r.get::<_,i64>(0)? as u64)).optional().map_err(internal)?;
        let Some(run) = run else {
            return Ok(Err(ResumeDelivery::NotDue));
        };
        let context = self
            .context(RunId::new(run))?
            .ok_or_else(|| ApiError::not_found("run"))?;
        // Acceptance and delivery answer the same rule, so an intent
        // accepted while custody held is closed rather than delivered
        // once a terminal Ticket, an archived Project, a supersession,
        // a result, or a settled authority overtook it (KAN-T142-AC2).
        if context.admit_resume().is_err() {
            span.execute(
                "UPDATE run_resume_deliveries SET status='obsolete' WHERE recovery_id=?1",
                [integer(id)?],
            )
            .map_err(internal)?;
            span.commit().map_err(internal)?;
            return Ok(Err(ResumeDelivery::Closed));
        }
        span.execute("UPDATE run_resume_deliveries SET attempts=attempts+1,next_attempt_at=unixepoch()+1 WHERE recovery_id=?1", [integer(id)?]).map_err(internal)?;
        span.commit().map_err(internal)?;
        Ok(Ok(PendingRunResume {
            recovery_id: id,
            run_id: run,
            dispatch_request_id: context.dispatch_request_id,
        }))
    }

    /// Close an accepted prompt's intent. The `pending` predicate is
    /// what keeps an intent some other disposition already closed
    /// from being rewritten as delivered.
    fn acknowledge(&self, id: u64) -> Result<(), ApiError> {
        self.conn.lock().execute("UPDATE run_resume_deliveries SET status='delivered' WHERE recovery_id=?1 AND status='pending'",[integer(id)?]).map_err(internal)?;
        Ok(())
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
        if action == RunRecoveryAction::Resume {
            context.admit_resume().map_err(refuse_resume)?;
            if context.pending_resume {
                return Err(ApiError::invalid_request(
                    "this run cannot accept another resume intent",
                ));
            }
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

/// The custody columns one context query reads, held raw so the
/// stored Ticket state is parsed outside the row closure.
struct ContextRow {
    project: u64,
    ticket: u64,
    version: u64,
    superseded: bool,
    has_submission: bool,
    dispatch_request_id: u64,
    resumable: bool,
    pending_resume: bool,
    ticket_state: String,
    project_archived: bool,
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
        // The Ticket and Project are joined rather than remembered:
        // a resume decided at any seam reads the lifecycle state and
        // the archival that hold now, not the ones the attempt began
        // under (KAN-T142).
        let row = conn.query_row(
            "SELECT runs.project_id, runs.ticket_id, (SELECT count(*) FROM run_recoveries WHERE run_id=runs.id),
               EXISTS(SELECT 1 FROM run_recoveries WHERE run_id=runs.id AND action='retry'),
               EXISTS(SELECT 1 FROM submissions WHERE run_id=runs.id) OR EXISTS(
                 SELECT 1 FROM review_slot_verdicts v JOIN dispatch_requests d ON d.reviewer_slot_id=v.slot_id WHERE d.id=runs.dispatch_request_id),
               runs.dispatch_request_id, EXISTS(SELECT 1 FROM capabilities c WHERE c.dispatch_request_id=runs.dispatch_request_id AND c.status='active' AND c.settled_at IS NULL),
               EXISTS(SELECT 1 FROM run_resume_deliveries d JOIN run_recoveries r ON r.id=d.recovery_id WHERE r.run_id=runs.id AND d.status='pending'),
               tickets.state, projects.archived
             FROM runs JOIN tickets ON tickets.id=runs.ticket_id
             JOIN projects ON projects.id=runs.project_id
             WHERE runs.id=?1", [integer(run.value())?], |row| Ok(ContextRow {
                project: row.get::<_, i64>(0)? as u64,
                ticket: row.get::<_, i64>(1)? as u64,
                version: row.get::<_, i64>(2)? as u64,
                superseded: row.get(3)?,
                has_submission: row.get(4)?,
                dispatch_request_id: row.get::<_,i64>(5)? as u64,
                resumable: row.get(6)?,
                pending_resume: row.get(7)?,
                ticket_state: row.get(8)?,
                project_archived: row.get(9)?,
            }),
        ).optional().map_err(internal)?;
        let Some(row) = row else {
            return Ok(None);
        };
        Ok(Some(RunRecoveryContext {
            project: ProjectId::new(row.project),
            ticket: TicketId::new(row.ticket),
            project_archived: row.project_archived,
            ticket_state: TicketState::parse(&row.ticket_state)
                .ok_or_else(|| ApiError::internal("stored Ticket state is invalid"))?,
            version: row.version,
            superseded: row.superseded,
            has_submission: row.has_submission,
            dispatch_request_id: row.dispatch_request_id,
            resumable: row.resumable,
            review_eligible: crate::review_execution::request_is_active(
                &conn,
                row.dispatch_request_id,
            )?,
            pending_resume: row.pending_resume,
        }))
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

    fn deliver_resume(
        &self,
        id: u64,
        dispatch: &mut dyn FnMut(&PendingRunResume) -> ResumeDispatch,
    ) -> Result<ResumeDelivery, ApiError> {
        // Held from the custody check to the acknowledgement. Every
        // mutation waits for it, so nothing can invalidate this
        // delivery between the state it was authorised against and
        // the prompt it sends; the connection is released before the
        // dispatch, so the transport call holds no database lock
        // (KAN-T142-AC2).
        let _authorised = self.delivery_gate.enter();
        let authorised = match self.authorise(id)? {
            Ok(authorised) => authorised,
            Err(refused) => return Ok(refused),
        };
        match dispatch(&authorised) {
            ResumeDispatch::Accepted => {
                self.acknowledge(id)?;
                Ok(ResumeDelivery::Delivered)
            }
            // A refusal and an ambiguous transport failure both leave
            // the intent pending for its bounded retry: neither is
            // evidence the Coordinator took the work.
            ResumeDispatch::Declined => Ok(ResumeDelivery::Declined),
            ResumeDispatch::Failed => Ok(ResumeDelivery::Failed),
        }
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

#[cfg(test)]
mod resume_eligibility {
    use kanban_app::RunRecoveryStore;
    use kanban_app::run_recovery::{ResumeDelivery, ResumeDispatch};
    use kanban_domain::{RulingSummary, RunId};

    use super::SqliteRunRecoveryStore;
    use crate::db::Database;
    use crate::migrations::AllowAllMigrations;
    use crate::test_support::scratch_database;

    /// One accepted implementer attempt: an active Project, a ready
    /// Ticket, an open Dispatch Request, the active authority that
    /// claim minted, and the Run it acknowledged. This is the state
    /// recovery's resume is legitimately offered from.
    fn attempt() -> (tempfile::TempDir, Database, SqliteRunRecoveryStore) {
        let (dir, mut database) = scratch_database();
        database
            .migrate(&AllowAllMigrations)
            .expect("the migrations apply");
        database
            .connection()
            .execute_batch(
                "INSERT INTO projects
                     (id, code, name, repository, seed_workspace, default_branch,
                      herdr_workspace, herdr_session, archived, version)
                 VALUES (1, 'CORE', 'Control plane', '/repositories/kanban',
                         '/workspaces/kanban.seed', 'main', 'kanban.seed',
                         'kanban-main', 0, 1);
                 INSERT INTO execution_profiles
                     (name, harness, model, effort, usage_pool, version)
                 VALUES ('standard', 'claude-code', 'opus', 'high', 'operator', 1);
                 INSERT INTO tickets
                     (id, project_id, number, kind, priority, state, title, criteria,
                      subtype, mode, completion, profile, version)
                 VALUES (1, 1, 1, 'task', 'normal', 'ready', 'One slice', '[]',
                         'operational', 'agent', '[\"done\"]', 'standard', 1);
                 INSERT INTO lanes (id, project_id, ticket_id, version) VALUES (1, 1, 1, 1);
                 INSERT INTO dispatch_requests
                     (id, project_id, ticket_id, status, priority, ready,
                      harness, model, usage_pool, created_at, version)
                 VALUES (1, 1, 1, 'claimed', 'normal', 1, 'claude-code', 'opus',
                         'operator', 0, 1);
                 INSERT INTO capabilities
                     (id, dispatch_request_id, ticket_id, lane_id, role, operations,
                      status, minted_at)
                 VALUES (1, 1, 1, 1, 'implementer', '[]', 'active', 0);
                 INSERT INTO runs
                     (id, project_id, ticket_id, dispatch_request_id, status,
                      requested_name, requested_harness, requested_model,
                      requested_effort, requested_usage_pool,
                      effective_name, effective_harness, effective_model,
                      effective_effort, effective_usage_pool,
                      fallback, created_at, version)
                 VALUES (1, 1, 1, 1, 'executing', 'standard', 'claude-code', 'opus',
                         'high', 'operator', 'standard', 'claude-code', 'opus',
                         'high', 'operator', 0, 0, 1);",
            )
            .expect("the fixture attempt lands");
        let store = SqliteRunRecoveryStore::new(&database);
        (dir, database, store)
    }

    /// The rows the production lifecycle and Project commands write
    /// when the operator makes a terminal change.
    fn invalidate(database: &Database, invalidation: &str) {
        let statement = match invalidation {
            "cancelled" => "UPDATE tickets SET state='cancelled', version=version+1 WHERE id=1",
            "archived" => "UPDATE projects SET archived=1, version=version+1 WHERE id=1",
            other => panic!("unknown invalidation {other}"),
        };
        database
            .connection()
            .execute(statement, [])
            .expect("the invalidation lands");
    }

    fn summary(text: &str) -> RulingSummary {
        RulingSummary::new(text).expect("the fixture summary validates")
    }

    fn count(database: &Database, table: &str) -> i64 {
        database
            .connection()
            .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .expect("the count reads")
    }

    fn delivery(database: &Database) -> (String, i64) {
        database
            .connection()
            .query_row(
                "SELECT status, attempts FROM run_resume_deliveries WHERE recovery_id=1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("the delivery row reads")
    }

    /// KAN-T142-AC1: the custody projection reads the Ticket's current
    /// state and the Project's current archival, so `can_resume`
    /// answers authoritative state rather than authority alone.
    #[test]
    fn context_reads_the_current_terminal_ticket_state_and_project_archival() {
        for invalidation in ["cancelled", "archived"] {
            let (_dir, database, store) = attempt();
            let eligible = store
                .context(RunId::new(1))
                .expect("the context reads")
                .expect("the fixture Run exists");
            assert!(
                eligible.can_resume(),
                "{invalidation}: the eligible attempt starts resumable"
            );

            invalidate(&database, invalidation);

            let context = store
                .context(RunId::new(1))
                .expect("the context reads")
                .expect("the fixture Run exists");
            assert!(
                !context.can_resume(),
                "{invalidation}: current state refuses resume"
            );
            assert!(
                context.admit_resume().is_err(),
                "{invalidation}: the refusal is typed"
            );
        }
    }

    /// KAN-T142-AC1: the store refuses to record a resume intent for a
    /// terminal Ticket or an archived Project, and writes no recovery
    /// row, no Ruling, and no delivery intent while doing so.
    #[test]
    fn recording_a_resume_is_refused_and_writes_nothing_for_terminal_custody() {
        for invalidation in ["cancelled", "archived"] {
            let (_dir, database, store) = attempt();
            invalidate(&database, invalidation);

            let refusal = store
                .resume(RunId::new(1), summary("Resume the original attempt"))
                .expect_err("terminal custody refuses a resume record");

            assert_eq!(
                refusal.code,
                kanban_dto::ErrorCode::InvalidRequest,
                "{invalidation}: {refusal:?}"
            );
            for table in ["run_recoveries", "rulings", "run_resume_deliveries"] {
                assert_eq!(
                    count(&database, table),
                    0,
                    "{invalidation}: the refusal writes no {table} row"
                );
            }
        }
    }

    /// Deliver one intent with a scripted transport, reporting what
    /// the delivery returned and how many prompts the authorisation
    /// actually admitted.
    fn deliver(
        store: &SqliteRunRecoveryStore,
        id: u64,
        reported: ResumeDispatch,
    ) -> (ResumeDelivery, usize) {
        let mut dispatched = 0;
        let outcome = store
            .deliver_resume(id, &mut |authorised| {
                assert_eq!(authorised.recovery_id, id);
                dispatched += 1;
                reported
            })
            .expect("the delivery reads");
        (outcome, dispatched)
    }

    /// KAN-T142-AC2: an intent accepted while custody was still valid
    /// is checked again at delivery. The delivery is refused and
    /// closed as obsolete without dispatching anything, and the
    /// operator's own change and the recovery audit both survive
    /// untouched.
    #[test]
    fn delivery_rechecks_current_custody_and_closes_the_obsolete_intent() {
        for invalidation in ["cancelled", "archived"] {
            let (_dir, database, store) = attempt();
            store
                .resume(RunId::new(1), summary("Resume while custody allows it"))
                .expect("the eligible attempt accepts the intent");
            assert_eq!(delivery(&database), ("pending".to_owned(), 0));

            invalidate(&database, invalidation);

            assert_eq!(
                deliver(&store, 1, ResumeDispatch::Accepted),
                (ResumeDelivery::Closed, 0),
                "{invalidation}: delivery is refused on current state, and prompts nobody"
            );
            assert_eq!(
                delivery(&database),
                ("obsolete".to_owned(), 0),
                "{invalidation}: the refused intent is closed, never attempted"
            );
            assert_eq!(count(&database, "run_recoveries"), 1, "{invalidation}");
            assert_eq!(count(&database, "rulings"), 1, "{invalidation}");
            assert_eq!(count(&database, "runs"), 1, "{invalidation}");
            let (state, archived): (String, i64) = database
                .connection()
                .query_row(
                    "SELECT t.state, p.archived FROM tickets t JOIN projects p ON p.id=t.project_id
                     WHERE t.id=1",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .expect("the current state reads");
            match invalidation {
                "cancelled" => assert_eq!(state, "cancelled"),
                _ => assert_eq!(archived, 1),
            }
        }
    }

    /// KAN-T142-AC3: an accepted prompt closes the intent as
    /// delivered, and no later pass dispatches a second one.
    #[test]
    fn an_accepted_prompt_closes_the_intent_and_delivers_no_second_time() {
        let (_dir, database, store) = attempt();
        store
            .resume(RunId::new(1), summary("Resume the recovered attempt"))
            .expect("the eligible attempt accepts the intent");

        assert_eq!(
            deliver(&store, 1, ResumeDispatch::Accepted),
            (ResumeDelivery::Delivered, 1)
        );
        assert_eq!(delivery(&database), ("delivered".to_owned(), 1));

        assert_eq!(
            deliver(&store, 1, ResumeDispatch::Accepted),
            (ResumeDelivery::NotDue, 0),
            "a closed intent dispatches no second prompt"
        );
        assert_eq!(delivery(&database), ("delivered".to_owned(), 1));
        assert_eq!(count(&database, "run_recoveries"), 1);
        assert_eq!(count(&database, "runs"), 1);
    }

    /// KAN-T142-AC3: a Coordinator that answers and refuses, and a
    /// transport that fails ambiguously, both leave the intent
    /// pending behind its bounded retry delay. Neither is recorded as
    /// a delivery.
    #[test]
    fn a_refused_prompt_and_a_failed_transport_both_leave_the_intent_pending() {
        for reported in [ResumeDispatch::Declined, ResumeDispatch::Failed] {
            let (_dir, database, store) = attempt();
            store
                .resume(RunId::new(1), summary("Resume the recovered attempt"))
                .expect("the eligible attempt accepts the intent");

            let (outcome, dispatched) = deliver(&store, 1, reported);

            assert_eq!(dispatched, 1, "{reported:?}");
            assert_eq!(
                outcome,
                match reported {
                    ResumeDispatch::Declined => ResumeDelivery::Declined,
                    _ => ResumeDelivery::Failed,
                },
                "{reported:?}: the transport's own answer is reported, never upgraded"
            );
            assert_eq!(
                delivery(&database),
                ("pending".to_owned(), 1),
                "{reported:?}: the intent stays pending and claims no delivery"
            );
            assert_eq!(
                deliver(&store, 1, ResumeDispatch::Accepted),
                (ResumeDelivery::NotDue, 0),
                "{reported:?}: the bounded retry delay holds the next attempt back"
            );
        }
    }

    /// The authorisation is free again: another holder takes it
    /// without waiting. A delivery that left it held would stall
    /// every Core mutation behind it.
    fn assert_authorisation_released(database: &Database, disposition: &str) {
        let gate = database.delivery_gate();
        let (entered, waiting) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _held = gate.enter();
            let _ = entered.send(());
        });
        waiting
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap_or_else(|_| panic!("{disposition}: the authorisation is still held"));
    }

    /// KAN-T142-AC2: every way out of a delivery releases the
    /// authorisation, not only the accepted one. A refusal, an
    /// ambiguous transport failure, an intent current custody closed,
    /// and one still inside its retry delay all leave the gate free,
    /// so no disposition can hold an unrelated command behind it.
    #[test]
    fn every_delivery_disposition_releases_the_authorisation() {
        for reported in [
            ResumeDispatch::Accepted,
            ResumeDispatch::Declined,
            ResumeDispatch::Failed,
        ] {
            let (_dir, database, store) = attempt();
            store
                .resume(RunId::new(1), summary("Resume the recovered attempt"))
                .expect("the eligible attempt accepts the intent");

            deliver(&store, 1, reported);

            assert_authorisation_released(&database, &format!("{reported:?}"));
        }

        let (_dir, database, store) = attempt();
        store
            .resume(RunId::new(1), summary("Resume the recovered attempt"))
            .expect("the eligible attempt accepts the intent");
        invalidate(&database, "cancelled");
        assert_eq!(
            deliver(&store, 1, ResumeDispatch::Accepted).0,
            ResumeDelivery::Closed
        );
        assert_authorisation_released(&database, "Closed");
        assert_eq!(
            deliver(&store, 1, ResumeDispatch::Accepted).0,
            ResumeDelivery::NotDue
        );
        assert_authorisation_released(&database, "NotDue");
    }

    /// KAN-T142-AC2: acknowledgement records that an authorised
    /// prompt was accepted. It is not a second chance to close an
    /// intent some other disposition already closed, so a row that is
    /// no longer pending is never rewritten as delivered.
    #[test]
    fn an_acknowledgement_never_reopens_an_intent_another_disposition_closed() {
        let (_dir, database, store) = attempt();
        store
            .resume(RunId::new(1), summary("Resume the recovered attempt"))
            .expect("the eligible attempt accepts the intent");

        let outcome = store
            .deliver_resume(1, &mut |_| {
                database
                    .connection()
                    .execute(
                        "UPDATE run_resume_deliveries SET status='obsolete' WHERE recovery_id=1",
                        [],
                    )
                    .expect("the competing disposition lands");
                ResumeDispatch::Accepted
            })
            .expect("the delivery reads");

        assert_eq!(outcome, ResumeDelivery::Delivered);
        assert_eq!(
            delivery(&database).0,
            "obsolete",
            "an intent another disposition closed is never acknowledged as delivered"
        );
    }

    /// KAN-T142-AC2: the pending-intent query is a Project-scoped
    /// queue, so the refusal belongs at the delivery authorisation
    /// where current custody is read, not in the queue that finds the
    /// work.
    #[test]
    fn pending_intents_stay_queued_until_delivery_reads_current_custody() {
        let (_dir, database, store) = attempt();
        store
            .resume(RunId::new(1), summary("Resume while custody allows it"))
            .expect("the eligible attempt accepts the intent");
        invalidate(&database, "cancelled");

        assert_eq!(
            store
                .pending_resume_ids(kanban_domain::ProjectId::new(1))
                .expect("the pending queue reads"),
            vec![1],
        );
        assert_eq!(
            deliver(&store, 1, ResumeDispatch::Accepted),
            (ResumeDelivery::Closed, 0)
        );
        assert!(
            store
                .pending_resume_ids(kanban_domain::ProjectId::new(1))
                .expect("the pending queue reads")
                .is_empty(),
            "a closed intent leaves the queue"
        );
    }
}
