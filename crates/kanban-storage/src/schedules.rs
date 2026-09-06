//! The SQLite implementation of the Schedule storage port: rows in
//! `schedules` holding one scheduled activation — a one-time
//! Schedule's trigger instant, timezone, eligible profile, and next
//! activation (DR-SA-01) — with the Ticket row it holds and the
//! timeline envelope landing unchanged in the same transaction as
//! every change. Attaching guards the Ticket's version; the due scan
//! reads waiting one-time Schedules in activation order; firing
//! spends the Schedule under its waiting state, moves the Ticket row
//! the domain activated, and appends the audit row, all in one write,
//! so a fired activation can never land twice (DR-SA-06).

use kanban_app::{DueActivation, ScheduleStore, TimelineEnvelope};
use kanban_domain::{Schedule, ScheduleId, ScheduleState, ScheduleTrigger, Ticket};
use kanban_dto::ApiError;
use rusqlite::params;

use crate::db::{ConnectionHandle, Database, WriteSpan};
use crate::projects::{PROJECT_COLUMNS, decode_row_at};
use crate::tickets::{TICKET_COLUMNS, load_ticket_row, move_row};
use crate::timeline::insert_event;

/// Every stored column of one Schedule row after the joined Ticket
/// and Project columns, in select order.
const SCHEDULE_COLUMNS: &str = "s.id, s.trigger_kind, s.activation_at, s.cron_expression, \
                                s.timezone, s.profile, s.next_activation, s.state";

/// The Schedule port over the authoritative database.
pub struct SqliteScheduleStore {
    conn: ConnectionHandle,
}

impl SqliteScheduleStore {
    /// Share the connection the `database` owns.
    pub fn new(database: &Database) -> Self {
        Self {
            conn: database.connection_handle(),
        }
    }

    /// Lock the shared connection.
    fn lock(&self) -> parking_lot::ReentrantMutexGuard<'_, rusqlite::Connection> {
        self.conn.lock()
    }
}

impl ScheduleStore for SqliteScheduleStore {
    fn attach(
        &self,
        ticket: &Ticket,
        schedule: &Schedule,
        envelope: &dyn Fn(ScheduleId) -> TimelineEnvelope,
    ) -> Result<Schedule, ApiError> {
        let conn = self.lock();
        let span = WriteSpan::begin(&conn).map_err(internal)?;
        move_row(&span, ticket)?;
        let (kind, activation_at, cron_expression) = stored_trigger(schedule.trigger());
        span.execute(
            "INSERT INTO schedules
                 (ticket_id, trigger_kind, activation_at, cron_expression, timezone,
                  profile, next_activation, state)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'waiting')",
            params![
                schedule.ticket().value() as i64,
                kind,
                activation_at,
                cron_expression,
                schedule.timezone().as_str(),
                schedule.profile().as_str(),
                schedule.next_activation(),
            ],
        )
        .map_err(internal)?;
        let id = ScheduleId::new(
            span.last_insert_rowid()
                .try_into()
                .map_err(|_| ApiError::internal("the Schedule identity overflowed"))?,
        );
        append_timeline(&span, &envelope(id))?;
        span.commit().map_err(internal)?;
        Ok(identified(schedule, id))
    }

    fn due(&self, now: &str) -> Result<Vec<DueActivation>, ApiError> {
        let conn = self.lock();
        let mut statement = conn
            .prepare(&format!(
                "SELECT {ticket_columns}, {project_columns}, {SCHEDULE_COLUMNS}
                 FROM schedules AS s
                 JOIN tickets AS t ON t.id = s.ticket_id
                 JOIN projects AS p ON p.id = t.project_id
                 WHERE s.state = 'waiting'
                   AND s.trigger_kind = 'one_time'
                   AND s.next_activation <= ?1
                 ORDER BY s.next_activation, s.id",
                ticket_columns = qualified(TICKET_COLUMNS, "t"),
                project_columns = qualified(PROJECT_COLUMNS, "p"),
            ))
            .map_err(internal)?;
        let rows = statement
            .query_map(params![now], load_due_row)
            .map_err(internal)?;
        let mut due = Vec::new();
        for row in rows {
            due.push(row.map_err(internal)?);
        }
        Ok(due)
    }

    fn fire(
        &self,
        due: &DueActivation,
        moved: Option<&Ticket>,
        fired_at: &str,
        envelope: TimelineEnvelope,
    ) -> Result<bool, ApiError> {
        let conn = self.lock();
        let span = WriteSpan::begin(&conn).map_err(internal)?;
        let changed = span
            .execute(
                "UPDATE schedules
                 SET state = 'fired', fired_at = ?2, version = version + 1
                 WHERE id = ?1 AND state = 'waiting'",
                params![due.id().value() as i64, fired_at],
            )
            .map_err(internal)?;
        if changed != 1 {
            // Another writer spent this activation; everything this
            // span wrote rolls back with it.
            return Ok(false);
        }
        if let Some(ticket) = moved {
            move_row(&span, ticket)?;
        }
        insert_event(&span, &envelope).map_err(timeline_failed)?;
        span.commit().map_err(internal)?;
        Ok(true)
    }
}

/// Insert the application's envelope, unchanged, on the same
/// transaction as the row it records.
fn append_timeline(
    conn: &rusqlite::Connection,
    envelope: &TimelineEnvelope,
) -> Result<(), ApiError> {
    insert_event(conn, envelope).map_err(timeline_failed)
}

/// Report a timeline insert failure the caller cannot act on.
fn timeline_failed(error: crate::error::StorageError) -> ApiError {
    ApiError::internal(&error.to_string())
}

/// Decode one due-activation row: the Ticket first, the Project
/// beside it, the Schedule last. Every stored value passed domain
/// validation on the way in, so a row that fails to rehydrate is
/// corruption the caller must hear about.
fn load_due_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<DueActivation> {
    let ticket = load_ticket_row(row)?.rehydrate()?;
    let project = decode_row_at(row, 21)?;
    let id = row.get::<_, i64>(35)?.unsigned_abs();
    let trigger_kind: String = row.get(36)?;
    let activation_at: Option<String> = row.get(37)?;
    let cron_expression: Option<String> = row.get(38)?;
    let timezone: String = row.get(39)?;
    let profile: String = row.get(40)?;
    let next_activation: String = row.get(41)?;
    let state: String = row.get(42)?;
    let trigger = rehydrate_trigger(&trigger_kind, activation_at, cron_expression)?;
    let schedule = Schedule::restore(
        ScheduleId::new(id),
        ticket.id(),
        trigger,
        kanban_domain::Timezone::new(&timezone).map_err(|_| corrupt())?,
        kanban_domain::ProfileName::new(&profile).map_err(|_| corrupt())?,
        next_activation,
        ScheduleState::parse(&state).ok_or_else(corrupt)?,
    );
    Ok(DueActivation {
        schedule,
        ticket,
        project,
    })
}

/// The trigger a stored row names, or corruption when the row holds
/// a shape the closed vocabulary refuses.
fn rehydrate_trigger(
    kind: &str,
    activation_at: Option<String>,
    cron_expression: Option<String>,
) -> rusqlite::Result<kanban_domain::ScheduleTrigger> {
    match (kind, activation_at, cron_expression) {
        ("one_time", Some(activation), None) => {
            Ok(kanban_domain::ScheduleTrigger::OneTime { activation })
        }
        ("cron", None, Some(expression)) => Ok(kanban_domain::ScheduleTrigger::Recurring {
            expression: kanban_domain::CronExpression::new(&expression).map_err(|_| corrupt())?,
        }),
        _ => Err(corrupt()),
    }
}

/// The SQLite failure a corrupt row reports.
fn corrupt() -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(CorruptRow))
}

/// A stored Schedule row failed domain validation.
#[derive(Debug)]
struct CorruptRow;

impl std::fmt::Display for CorruptRow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "a stored Schedule row failed validation")
    }
}

impl std::error::Error for CorruptRow {}

/// One column list qualified with `alias`, for the joined reads.
fn qualified(columns: &str, alias: &str) -> String {
    columns
        .split(", ")
        .map(|column| format!("{alias}.{column}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// The stored form of one Schedule's trigger.
fn stored_trigger(trigger: &ScheduleTrigger) -> (&'static str, Option<String>, Option<String>) {
    match trigger {
        ScheduleTrigger::OneTime { activation } => ("one_time", Some(activation.clone()), None),
        ScheduleTrigger::Recurring { expression } => {
            ("cron", None, Some(expression.as_str().to_owned()))
        }
    }
}

/// The stored Schedule the caller assembled, with the identity
/// storage assigned it.
fn identified(schedule: &Schedule, id: ScheduleId) -> Schedule {
    Schedule::restore(
        id,
        schedule.ticket(),
        schedule.trigger().clone(),
        schedule.timezone().clone(),
        schedule.profile().clone(),
        schedule.next_activation().to_owned(),
        schedule.state(),
    )
}

/// Report a SQLite failure the caller cannot act on.
fn internal(error: rusqlite::Error) -> ApiError {
    ApiError::internal(&error.to_string())
}

#[cfg(test)]
mod tests {
    use kanban_app::{ProjectStore, ScheduleStore, TicketStore, TimelineEnvelope};
    use kanban_domain::{
        NumberKind, Priority, Project, ProjectId, ProjectRegistration, Schedule, ScheduleId,
        TaskMode, TaskSubtype, TaskTiming, Ticket, TicketBody, TicketId, TicketNumber, TicketState,
    };
    use kanban_dto::{ErrorCode, TimelineEntityKind, TimelineEntityRef, TimelineEventKind};
    use serde_json::json;

    use super::SqliteScheduleStore;
    use crate::db::Database;
    use crate::migrations::AllowAllMigrations;
    use crate::projects::SqliteProjectStore;
    use crate::test_support::scratch_database;
    use crate::tickets::SqliteTicketStore;

    /// The instant every due fixture activates at, in the stored
    /// shape.
    const ACTIVATION: &str = "2026-09-10T09:00:00.000Z";

    /// A later moment the due scans read.
    const NOW: &str = "2026-09-11T00:00:00.000Z";

    fn store() -> (
        tempfile::TempDir,
        Database,
        SqliteScheduleStore,
        SqliteTicketStore,
    ) {
        let (dir, mut database) = scratch_database();
        database
            .migrate(&AllowAllMigrations)
            .expect("the migrations apply");
        let schedules = SqliteScheduleStore::new(&database);
        let tickets = SqliteTicketStore::new(&database);
        (dir, database, schedules, tickets)
    }

    fn registration() -> ProjectRegistration {
        ProjectRegistration::new(
            "CORE",
            "Control plane",
            "/repositories/kanban",
            "/workspaces/kanban.seed",
            "main",
            "kanban.seed",
            Some("kanban-main"),
            None,
        )
        .expect("the fixture registration validates")
    }

    /// Seed the Project row the Tickets write against.
    fn seeded_project(database: &Database) -> Project {
        let projects = SqliteProjectStore::new(database);
        projects
            .create(&registration(), &|id| {
                TimelineEnvelope::project(
                    id.value(),
                    TimelineEventKind::Transition,
                    Some(TimelineEntityRef {
                        kind: TimelineEntityKind::Project,
                        id: id.value().to_string(),
                    }),
                    json!({ "action": "registered", "id": id.value() }),
                )
            })
            .expect("the fixture Project lands")
    }

    /// Create one Task Ticket through the port and return the stored
    /// aggregate.
    fn created_task(database: &Database) -> Ticket {
        let projects = SqliteProjectStore::new(database);
        let mut project = projects
            .find(ProjectId::new(1))
            .expect("the reload serves")
            .expect("the Project exists");
        let number = TicketNumber::new(project.mint(NumberKind::Ticket).expect("active mints"))
            .expect("a minted number is positive");
        let body = TicketBody::task(
            "Archive the old register",
            None,
            Some(TaskSubtype::Operational),
            Some(TaskMode::Human),
            vec![
                kanban_domain::CompletionCriterion::new("The register is archived.")
                    .expect("the fixture outcome binds"),
            ],
            TaskTiming::none(),
        )
        .expect("the fixture body validates");
        SqliteTicketStore::new(database)
            .create(&project, number, Priority::Normal, &body, &|id| {
                transition_envelope(id, "created", json!({ "from": "none", "to": "draft" }))
            })
            .expect("the fixture Ticket lands")
    }

    /// One waiting one-time Schedule bound to `ticket`.
    fn schedule_for(ticket: TicketId, activation: &str) -> Schedule {
        Schedule::one_time(ticket, activation, "Europe/Amsterdam", "standard")
            .expect("the fixture schedule validates")
    }

    /// The Ticket moved to `state`, counting the change.
    fn moved_to(ticket: &Ticket, state: TicketState) -> Ticket {
        Ticket::restore(
            ticket.id(),
            ticket.project(),
            ticket.number(),
            ticket.priority(),
            state,
            ticket.body().clone(),
            ticket.profile().cloned(),
            ticket.version() + 1,
        )
    }

    /// The timeline envelope one Ticket transition lands.
    fn transition_envelope(
        ticket: TicketId,
        action: &str,
        facts: serde_json::Value,
    ) -> TimelineEnvelope {
        let mut detail = facts;
        let object = detail.as_object_mut().expect("the facts are an object");
        object.insert("action".to_owned(), serde_json::Value::from(action));
        object.insert("id".to_owned(), serde_json::Value::from(ticket.value()));
        TimelineEnvelope::project(
            1,
            TimelineEventKind::Transition,
            Some(TimelineEntityRef {
                kind: TimelineEntityKind::Ticket,
                id: ticket.value().to_string(),
            }),
            detail,
        )
    }

    /// Every ticket-scoped timeline row's detail, in landing order.
    fn ticket_timeline(database: &Database) -> Vec<serde_json::Value> {
        let conn = database.connection();
        let mut statement = conn
            .prepare(
                "SELECT detail FROM timeline_events
                 WHERE scope = 'project' AND entity_kind = 'ticket' ORDER BY id",
            )
            .expect("the timeline is readable");
        statement
            .query_map([], |row| {
                let detail: String = row.get(0)?;
                Ok(serde_json::from_str(&detail).expect("stored detail is JSON"))
            })
            .expect("the query runs")
            .collect::<Result<Vec<_>, _>>()
            .expect("the rows decode")
    }

    /// The stored Schedule row as plain data, for assertions.
    fn stored_schedule(
        database: &Database,
        id: i64,
    ) -> (String, Option<String>, String, String, String, String) {
        database
            .connection()
            .query_row(
                "SELECT trigger_kind, activation_at, timezone, profile, next_activation, state
                 FROM schedules WHERE id = ?1",
                [id],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                    ))
                },
            )
            .expect("the schedule row reads")
    }

    #[test]
    fn attaching_lands_the_schedule_the_ticket_row_and_the_timeline_together() {
        let (_dir, database, schedules, tickets) = store();
        seeded_project(&database);
        let task = created_task(&database);
        let scheduled = moved_to(&task, TicketState::Scheduled);

        let landed = schedules
            .attach(&scheduled, &schedule_for(task.id(), ACTIVATION), &|id| {
                transition_envelope(
                    task.id(),
                    "scheduled",
                    json!({ "from": "draft", "to": "scheduled", "schedule": id.value() }),
                )
            })
            .expect("the schedule attaches");

        assert_eq!(landed.id(), Some(ScheduleId::new(1)));
        assert_eq!(
            stored_schedule(&database, 1),
            (
                "one_time".to_owned(),
                Some(ACTIVATION.to_owned()),
                "Europe/Amsterdam".to_owned(),
                "standard".to_owned(),
                ACTIVATION.to_owned(),
                "waiting".to_owned(),
            ),
            "the Schedule row lands whole (DR-SA-01)"
        );
        let ticket_row: (String, i64) = database
            .connection()
            .query_row(
                "SELECT state, version FROM tickets WHERE id = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("the Ticket row reads");
        assert_eq!(
            ticket_row,
            ("scheduled".to_owned(), 2),
            "the moved Ticket row lands in the same write"
        );
        assert_eq!(
            ticket_timeline(&database).last().expect("a row appended"),
            &json!({
                "action": "scheduled",
                "id": 1,
                "from": "draft",
                "to": "scheduled",
                "schedule": 1,
            }),
            "the envelope reaches the timeline unchanged, naming the Schedule"
        );
        let _ = tickets;
    }

    #[test]
    fn a_stale_attach_refuses_without_a_row_or_a_timeline_append() {
        let (_dir, database, schedules, _tickets) = store();
        seeded_project(&database);
        let task = created_task(&database);
        // The stored row stands at version 1 while the moved aggregate
        // claims to have moved twice.
        let stale = moved_to(
            &moved_to(&task, TicketState::Parked),
            TicketState::Scheduled,
        );
        let timeline_before = ticket_timeline(&database).len();

        let error = schedules
            .attach(&stale, &schedule_for(task.id(), ACTIVATION), &|id| {
                transition_envelope(task.id(), "scheduled", json!({ "schedule": id.value() }))
            })
            .expect_err("the stale attach is refused");

        assert_eq!(error.code, ErrorCode::StaleVersion);
        let rows: i64 = database
            .connection()
            .query_row("SELECT COUNT(*) FROM schedules", [], |row| row.get(0))
            .expect("the rows are readable");
        assert_eq!(rows, 0, "a refused attach lands no Schedule row");
        assert_eq!(
            ticket_timeline(&database).len(),
            timeline_before,
            "a refused attach appends no timeline row"
        );
    }

    #[test]
    fn due_lists_waiting_one_time_schedules_in_activation_order() {
        let (_dir, database, schedules, _tickets) = store();
        seeded_project(&database);
        let first = created_task(&database);
        let second = created_task(&database);
        schedules
            .attach(
                &moved_to(&first, TicketState::Scheduled),
                &schedule_for(first.id(), "2026-09-09T09:00:00Z"),
                &|id| {
                    transition_envelope(first.id(), "scheduled", json!({ "schedule": id.value() }))
                },
            )
            .expect("the earlier schedule attaches");
        schedules
            .attach(
                &moved_to(&second, TicketState::Scheduled),
                &schedule_for(second.id(), ACTIVATION),
                &|id| {
                    transition_envelope(second.id(), "scheduled", json!({ "schedule": id.value() }))
                },
            )
            .expect("the later schedule attaches");
        // A Schedule whose activation has not arrived waits quietly.
        let third = created_task(&database);
        schedules
            .attach(
                &moved_to(&third, TicketState::Scheduled),
                &schedule_for(third.id(), "2026-10-01T09:00:00Z"),
                &|id| {
                    transition_envelope(third.id(), "scheduled", json!({ "schedule": id.value() }))
                },
            )
            .expect("the future schedule attaches");

        let due = schedules.due(NOW).expect("the due scan serves");

        assert_eq!(
            due.iter()
                .map(|activation| activation.ticket.id().value())
                .collect::<Vec<_>>(),
            vec![1, 2],
            "the overdue activations answer in activation order, and the future one stays absent"
        );
        assert_eq!(due[0].schedule.id(), Some(ScheduleId::new(1)));
        assert_eq!(due[0].ticket.state(), TicketState::Scheduled);
        assert_eq!(due[0].project.code().as_str(), "CORE");
        assert!(due[0].schedule.is_due(NOW));
    }

    #[test]
    fn firing_spends_the_activation_moves_the_ticket_and_appends_once() {
        let (_dir, database, schedules, _tickets) = store();
        seeded_project(&database);
        let task = created_task(&database);
        schedules
            .attach(
                &moved_to(&task, TicketState::Scheduled),
                &schedule_for(task.id(), ACTIVATION),
                &|id| {
                    transition_envelope(task.id(), "scheduled", json!({ "schedule": id.value() }))
                },
            )
            .expect("the schedule attaches");
        let due = schedules
            .due(NOW)
            .expect("the scan serves")
            .pop()
            .expect("one is due");

        // The domain's activation rule moves the Ticket; the store
        // spends the Schedule and lands the move.
        let mut activated = due.ticket.clone();
        due.schedule
            .activate(&mut activated, &clear_readiness())
            .expect("the activation applies");
        let spent = schedules
            .fire(
                &due,
                Some(&activated),
                NOW,
                transition_envelope(
                    task.id(),
                    "activated",
                    json!({
                        "from": "scheduled", "to": "ready", "schedule": due.id().value(),
                    }),
                ),
            )
            .expect("the firing lands");

        assert!(spent);
        assert_eq!(
            stored_schedule(&database, 1),
            (
                "one_time".to_owned(),
                Some(ACTIVATION.to_owned()),
                "Europe/Amsterdam".to_owned(),
                "standard".to_owned(),
                ACTIVATION.to_owned(),
                "fired".to_owned(),
            ),
            "the spent Schedule is fired, never due again"
        );
        let ticket_row: (String, i64) = database
            .connection()
            .query_row(
                "SELECT state, version FROM tickets WHERE id = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("the Ticket row reads");
        assert_eq!(
            ticket_row,
            ("ready".to_owned(), 3),
            "the activated Ticket row moved in the same write"
        );
        assert_eq!(
            ticket_timeline(&database).last().expect("a row appended"),
            &json!({
                "action": "activated",
                "id": 1,
                "from": "scheduled",
                "to": "ready",
                "schedule": 1,
            })
        );
        assert!(
            schedules.due(NOW).expect("the rescan serves").is_empty(),
            "a fired one-time Schedule is never due again (DR-SA-06)"
        );

        // A second fire of the same due activation spends nothing.
        let again = schedules
            .fire(
                &due,
                Some(&activated),
                NOW,
                transition_envelope(task.id(), "activated", json!({})),
            )
            .expect("the second fire answers");
        assert!(!again, "another writer already spent the activation");
        let ticket_row: (String, i64) = database
            .connection()
            .query_row(
                "SELECT state, version FROM tickets WHERE id = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("the Ticket row reads");
        assert_eq!(ticket_row, ("ready".to_owned(), 3), "nothing moved twice");
    }

    #[test]
    fn firing_an_already_circulating_ticket_spends_the_schedule_without_a_row_move() {
        let (_dir, database, schedules, tickets) = store();
        seeded_project(&database);
        let task = created_task(&database);
        schedules
            .attach(
                &moved_to(&task, TicketState::Scheduled),
                &schedule_for(task.id(), ACTIVATION),
                &|id| {
                    transition_envelope(task.id(), "scheduled", json!({ "schedule": id.value() }))
                },
            )
            .expect("the schedule attaches");
        // The Ticket reached ready by other means before its moment.
        let circulating = moved_to(&moved_to(&task, TicketState::Scheduled), TicketState::Ready);
        tickets
            .save(
                &circulating,
                transition_envelope(
                    task.id(),
                    "moved",
                    json!({ "from": "scheduled", "to": "ready" }),
                ),
            )
            .expect("the Ticket circulates");
        let due = schedules
            .due(NOW)
            .expect("the scan serves")
            .pop()
            .expect("one is due");

        let mut activated = due.ticket.clone();
        let outcome = due
            .schedule
            .activate(&mut activated, &clear_readiness())
            .expect("the activation answers");
        assert_eq!(outcome, kanban_domain::Activation::AlreadyCirculating);

        let spent = schedules
            .fire(
                &due,
                None,
                NOW,
                transition_envelope(
                    task.id(),
                    "activated",
                    json!({
                        "from": "ready", "to": "ready", "schedule": due.id().value(),
                    }),
                ),
            )
            .expect("the firing lands");

        assert!(spent);
        let ticket_row: (String, i64) = database
            .connection()
            .query_row(
                "SELECT state, version FROM tickets WHERE id = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("the Ticket row reads");
        assert_eq!(
            ticket_row,
            ("ready".to_owned(), 3),
            "an already-circulating Ticket moves nothing"
        );
        assert_eq!(
            stored_schedule(&database, 1).5,
            "fired".to_owned(),
            "the Schedule still spends"
        );
    }

    #[test]
    fn the_schema_keeps_a_schedule_to_its_own_trigger_columns() {
        let (_dir, database, _schedules, _tickets) = store();
        seeded_project(&database);
        created_task(&database);
        let conn = database.connection();

        for (sql, note) in [
            (
                "INSERT INTO schedules
                     (ticket_id, trigger_kind, activation_at, timezone, profile, next_activation)
                 VALUES (1, 'one_time', NULL, 'UTC', 'standard', '2026-09-10T09:00:00.000Z')",
                "a one-time Schedule names its activation instant",
            ),
            (
                "INSERT INTO schedules
                     (ticket_id, trigger_kind, activation_at, cron_expression, timezone, profile,
                      next_activation)
                 VALUES (1, 'one_time', '2026-09-10T09:00:00.000Z', '*/15 * * * *', 'UTC',
                         'standard', '2026-09-10T09:00:00.000Z')",
                "a one-time Schedule carries no cron expression",
            ),
            (
                "INSERT INTO schedules
                     (ticket_id, trigger_kind, activation_at, cron_expression, timezone, profile,
                      next_activation)
                 VALUES (1, 'cron', '2026-09-10T09:00:00.000Z', NULL, 'UTC', 'standard',
                         '2026-09-10T09:00:00.000Z')",
                "a cron Schedule names its expression",
            ),
            (
                "INSERT INTO schedules
                     (ticket_id, trigger_kind, activation_at, timezone, profile, next_activation)
                 VALUES (1, 'one_time', '2026-09-10T09:00:00.000Z', 'UTC', 'standard',
                         '2026-09-11T09:00:00.000Z')",
                "a one-time next activation is its activation",
            ),
            (
                "INSERT INTO schedules
                     (ticket_id, trigger_kind, activation_at, timezone, profile, next_activation)
                 VALUES (1, 'one_time', '2026-09-10T09:00:00.000Z', '  ', 'standard',
                         '2026-09-10T09:00:00.000Z')",
                "a Schedule names its timezone",
            ),
        ] {
            let error = conn.execute(sql, []).expect_err(note);
            assert!(
                error.to_string().contains("CHECK constraint failed"),
                "`{note}` should refuse, got: {error}"
            );
        }
        let rows: i64 = conn
            .query_row("SELECT COUNT(*) FROM schedules", [], |row| row.get(0))
            .expect("the rows are readable");
        assert_eq!(rows, 0, "every refused statement left the rows intact");
    }

    /// The clear readiness: nothing holds the Ticket back.
    fn clear_readiness() -> kanban_domain::Readiness {
        kanban_domain::compute_readiness(kanban_domain::ReadinessInputs {
            dependencies: &[],
            blockers: &[],
        })
    }
}
