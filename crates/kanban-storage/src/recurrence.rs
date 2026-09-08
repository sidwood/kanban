//! SQLite recurrence adapter; one write owns a whole occurrence advance.
use kanban_app::recurrence::{
    CommittedRecurrence, DueRecurrence, RecurrenceStore, occurrence_ticket, recurrence_envelopes,
};
use kanban_domain::recurrence::{CatchUpPolicy, RecurrenceAdvance};
use kanban_domain::{NumberKind, ScheduleId, Ticket, TicketId, TicketNumber};
use kanban_dto::ApiError;
use rusqlite::{OptionalExtension, params};

use crate::db::{ConnectionHandle, Database, WriteSpan};
use crate::projects::PROJECT_COLUMNS;
use crate::schedules::{SCHEDULE_COLUMNS, load_due_row, qualified};
use crate::tickets::TICKET_COLUMNS;
use crate::timeline::insert_event;

pub struct SqliteRecurrenceStore {
    conn: ConnectionHandle,
}
impl SqliteRecurrenceStore {
    pub fn new(database: &Database) -> Self {
        Self {
            conn: database.connection_handle(),
        }
    }
}

impl RecurrenceStore for SqliteRecurrenceStore {
    fn due(&self, now: &str) -> Result<Vec<ScheduleId>, ApiError> {
        let conn = self.conn.lock();
        let mut query = conn
            .prepare(
                "SELECT s.id FROM schedules s
            JOIN tickets t ON t.id = s.ticket_id JOIN projects p ON p.id = t.project_id
            WHERE s.trigger_kind = 'cron' AND s.state = 'waiting' AND s.next_activation <= ?1
                AND t.kind = 'task' AND t.state = 'scheduled' AND p.archived = 0
            ORDER BY s.next_activation, s.id",
            )
            .map_err(internal)?;
        query
            .query_map([now], |r| r.get::<_, i64>(0))
            .map_err(internal)?
            .map(|row| row.map(|id| ScheduleId::new(id as u64)).map_err(internal))
            .collect()
    }

    fn advance(
        &self,
        id: ScheduleId,
        now: &str,
        decide: &dyn Fn(&DueRecurrence) -> Result<Option<RecurrenceAdvance>, ApiError>,
    ) -> Result<Option<CommittedRecurrence>, ApiError> {
        let conn = self.conn.lock();
        // This scheduler pass owns a standalone write. Lock the writer before
        // reading the window, rather than upgrading a stale WAL snapshot.
        let span =
            rusqlite::Transaction::new_unchecked(&conn, rusqlite::TransactionBehavior::Immediate)
                .map_err(internal)?;
        let activation = span
            .query_row(
                &format!(
                    "SELECT {}, {}, {SCHEDULE_COLUMNS}
            FROM schedules s JOIN tickets t ON t.id = s.ticket_id
            JOIN projects p ON p.id = t.project_id
            WHERE s.id = ?1 AND s.trigger_kind = 'cron' AND s.state = 'waiting'
                AND s.next_activation <= ?2 AND t.kind = 'task' AND t.state = 'scheduled'
                AND p.archived = 0",
                    qualified(TICKET_COLUMNS, "t"),
                    qualified(PROJECT_COLUMNS, "p")
                ),
                params![id.value() as i64, now],
                load_due_row,
            )
            .optional()
            .map_err(internal)?;
        let Some(activation) = activation else {
            return Ok(None);
        };
        let has_open_occurrence: bool = span.query_row("SELECT EXISTS(
            SELECT 1 FROM task_occurrences o JOIN tickets t ON t.id = o.occurrence_ticket_id
            WHERE o.template_ticket_id = ?1 AND t.state NOT IN ('done', 'cancelled', 'superseded'))",
            [activation.ticket.id().value() as i64], |r| r.get(0)).map_err(internal)?;
        let policy = read_policy(&span, activation.project.id().value())?;
        let due = DueRecurrence {
            readiness: template_readiness(&span, activation.ticket.id())?,
            activation,
            catch_up: if policy.catch_up_one {
                CatchUpPolicy::One
            } else {
                CatchUpPolicy::Skip
            },
            has_open_occurrence,
        };
        let Some(decision) = decide(&due)? else {
            return Ok(None);
        };
        let ticket = if let Some(window) = &decision.occurrence_window {
            Some(insert_occurrence(&span, &due, window)?)
        } else {
            None
        };
        span.execute(
            "UPDATE schedules SET next_activation = ?2, version = version + 1 WHERE id = ?1",
            params![id.value() as i64, decision.next_activation],
        )
        .map_err(internal)?;
        if let Some(skipped) = &decision.skipped {
            span.execute("INSERT INTO schedule_attention
                (project_id, template_ticket_id, schedule_id, reason, first_window, last_window, next_activation, updated_at)
                VALUES (?1,?2,?3,?4,?5,?6,?7,?8)
                ON CONFLICT(template_ticket_id,reason) DO UPDATE SET
                    first_window = MIN(schedule_attention.first_window,excluded.first_window),
                    last_window = MAX(schedule_attention.last_window,excluded.last_window),
                    next_activation = excluded.next_activation, updated_at = excluded.updated_at",
                params![due.activation.project.id().value() as i64, due.activation.ticket.id().value() as i64,
                    due.activation.id().value() as i64, skipped.reason.wire_name(),
                    skipped.from, skipped.through, decision.next_activation, now]).map_err(internal)?;
        }
        for envelope in recurrence_envelopes(&due.activation, &decision, ticket.as_ref()) {
            insert_event(&span, &envelope).map_err(|e| ApiError::internal(&e.to_string()))?;
        }
        span.commit().map_err(internal)?;
        Ok(Some(CommittedRecurrence {
            activation: due.activation,
            ticket,
        }))
    }
}

fn insert_occurrence(
    conn: &rusqlite::Connection,
    due: &DueRecurrence,
    window: &str,
) -> Result<Ticket, ApiError> {
    let mut project = due.activation.project.clone();
    let number =
        TicketNumber::new(project.mint(NumberKind::Ticket).map_err(invalid)?).map_err(invalid)?;
    conn.execute(
        "UPDATE projects SET ticket_counter = ?2, version = ?3 WHERE id = ?1",
        params![
            project.id().value() as i64,
            number.value() as i64,
            project.version() as i64
        ],
    )
    .map_err(internal)?;
    conn.execute("INSERT INTO tickets
        (project_id, number, kind, priority, state, spec_id, title, criteria, subtype, mode, completion, version)
        SELECT project_id, ?2, 'task', priority, 'draft', spec_id, title, '[]', subtype, mode, completion, 1
        FROM tickets WHERE id = ?1", params![due.activation.ticket.id().value() as i64, number.value() as i64])
        .map_err(internal)?;
    // Capture the Ticket identity before the lineage and timeline allocate their own identities.
    let id = TicketId::new(
        conn.last_insert_rowid()
            .try_into()
            .map_err(|_| ApiError::internal("the Ticket identity overflowed"))?,
    );
    let ticket = occurrence_ticket(due, id, number)?;
    conn.execute(
        "UPDATE tickets SET state = ?2, profile = ?3, version = ?4 WHERE id = ?1",
        params![
            id.value() as i64,
            ticket.state().wire_name(),
            ticket.profile().map(|p| p.as_str()),
            ticket.version() as i64
        ],
    )
    .map_err(internal)?;
    conn.execute(
        "INSERT INTO ticket_dependencies (from_ticket,to_ticket)
        SELECT from_ticket, ?2 FROM ticket_dependencies WHERE to_ticket = ?1",
        params![due.activation.ticket.id().value() as i64, id.value() as i64],
    )
    .map_err(internal)?;
    conn.execute(
        "INSERT INTO ticket_blockers (ticket_id,description)
        SELECT ?2, description FROM ticket_blockers WHERE ticket_id = ?1",
        params![due.activation.ticket.id().value() as i64, id.value() as i64],
    )
    .map_err(internal)?;
    conn.execute(
        "INSERT INTO task_occurrences
        (schedule_id, template_ticket_id, occurrence_ticket_id, window_at) VALUES (?1, ?2, ?3, ?4)",
        params![
            due.activation.id().value() as i64,
            due.activation.ticket.id().value() as i64,
            id.value() as i64,
            window
        ],
    )
    .map_err(internal)?;
    Ok(ticket)
}

fn invalid(error: impl std::fmt::Display) -> ApiError {
    ApiError::invalid_request(&error.to_string())
}
fn internal(error: rusqlite::Error) -> ApiError {
    ApiError::internal(&error.to_string())
}

impl kanban_app::recurrence::SchedulePolicyStore for SqliteRecurrenceStore {
    fn list_attention(
        &self,
        project_id: u64,
    ) -> Result<kanban_dto::recurrence::ScheduleAttentionListResponse, ApiError> {
        use kanban_dto::recurrence::{ScheduleAttentionListResponse, ScheduleAttentionRecord};
        let conn = self.conn.lock();
        let mut query = conn
            .prepare(
                "SELECT id, project_id, template_ticket_id, schedule_id, reason,
            first_window, last_window, next_activation, updated_at FROM schedule_attention
            WHERE project_id = ?1 ORDER BY updated_at DESC, id",
            )
            .map_err(internal)?;
        let signals = query
            .query_map([project_id as i64], |r| {
                Ok(ScheduleAttentionRecord {
                    id: read_id(r, 0)?,
                    project_id: read_id(r, 1)?,
                    template_ticket_id: read_id(r, 2)?,
                    schedule_id: read_id(r, 3)?,
                    reason: serde_json::from_value(serde_json::Value::String(r.get(4)?)).map_err(
                        |e| {
                            rusqlite::Error::FromSqlConversionFailure(
                                4,
                                rusqlite::types::Type::Text,
                                Box::new(e),
                            )
                        },
                    )?,
                    first_window: r.get(5)?,
                    last_window: r.get(6)?,
                    next_activation: r.get(7)?,
                    updated_at: r.get(8)?,
                })
            })
            .map_err(internal)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(internal)?;
        Ok(ScheduleAttentionListResponse { signals })
    }

    fn get_policy(
        &self,
        project_id: u64,
    ) -> Result<kanban_dto::ProjectSchedulePolicyRecord, ApiError> {
        read_policy(&self.conn.lock(), project_id)
    }
    fn set_policy(
        &self,
        request: &kanban_dto::ProjectSchedulePolicySetRequest,
        envelope: &kanban_app::TimelineEnvelope,
    ) -> Result<kanban_dto::ProjectSchedulePolicyRecord, ApiError> {
        let conn = self.conn.lock();
        let span = WriteSpan::begin(&conn).map_err(internal)?;
        let active: bool = span
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM projects WHERE id = ?1 AND archived = 0)",
                [request.project_id as i64],
                |r| r.get(0),
            )
            .map_err(internal)?;
        if !active {
            return Err(ApiError::invalid_request(
                "scheduling policy requires a live Project",
            ));
        }
        let current = read_policy(&span, request.project_id)?;
        if request.mutation.optimistic_version != current.version {
            return Err(ApiError::stale_version(
                request.mutation.optimistic_version,
                current.version,
            ));
        }
        let saved = kanban_dto::ProjectSchedulePolicyRecord {
            project_id: request.project_id,
            catch_up_one: request.catch_up_one,
            version: current.version + 1,
        };
        span.execute("INSERT INTO project_schedule_policies (project_id, catch_up_one, version) VALUES (?1,?2,?3)
            ON CONFLICT(project_id) DO UPDATE SET catch_up_one = excluded.catch_up_one, version = excluded.version",
            params![saved.project_id as i64,saved.catch_up_one,saved.version as i64]).map_err(internal)?;
        insert_event(&span, envelope).map_err(|e| ApiError::internal(&e.to_string()))?;
        span.commit().map_err(internal)?;
        Ok(saved)
    }
}

fn read_policy(
    conn: &rusqlite::Connection,
    project_id: u64,
) -> Result<kanban_dto::ProjectSchedulePolicyRecord, ApiError> {
    Ok(conn
        .query_row(
            "SELECT catch_up_one, version FROM project_schedule_policies WHERE project_id = ?1",
            [project_id as i64],
            |r| {
                Ok(kanban_dto::ProjectSchedulePolicyRecord {
                    project_id,
                    catch_up_one: r.get(0)?,
                    version: r.get::<_, i64>(1)? as u64,
                })
            },
        )
        .optional()
        .map_err(internal)?
        .unwrap_or(kanban_dto::ProjectSchedulePolicyRecord {
            project_id,
            catch_up_one: false,
            version: 0,
        }))
}

fn read_id(row: &rusqlite::Row<'_>, column: usize) -> rusqlite::Result<u64> {
    u64::try_from(row.get::<_, i64>(column)?).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(
            column,
            rusqlite::types::Type::Integer,
            Box::new(e),
        )
    })
}

fn template_readiness(
    conn: &rusqlite::Connection,
    ticket: TicketId,
) -> Result<kanban_domain::Readiness, ApiError> {
    use kanban_domain::{
        BlockerDescription, DependencyState, ExternalBlocker, ExternalBlockerId, ReadinessInputs,
        TicketDependency, TicketState, compute_readiness,
    };
    let mut query = conn
        .prepare(
            "SELECT d.from_ticket,t.state FROM ticket_dependencies d
        JOIN tickets t ON t.id=d.from_ticket WHERE d.to_ticket=?1",
        )
        .map_err(internal)?;
    let rows = query
        .query_map([ticket.value() as i64], |r| {
            Ok((read_id(r, 0)?, r.get::<_, String>(1)?))
        })
        .map_err(internal)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(internal)?;
    let mut dependencies = Vec::new();
    for (from, state) in rows {
        dependencies.push(DependencyState {
            dependency: TicketDependency::new(TicketId::new(from), ticket),
            state: TicketState::parse(&state)
                .ok_or_else(|| ApiError::internal("stored dependency state is invalid"))?,
        });
    }
    let mut query = conn
        .prepare("SELECT id,description FROM ticket_blockers WHERE ticket_id=?1")
        .map_err(internal)?;
    let rows = query
        .query_map([ticket.value() as i64], |r| {
            Ok((read_id(r, 0)?, r.get::<_, String>(1)?))
        })
        .map_err(internal)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(internal)?;
    let mut blockers = Vec::new();
    for (id, description) in rows {
        blockers.push(ExternalBlocker::restore(
            ExternalBlockerId::new(id),
            ticket,
            BlockerDescription::new(&description).map_err(invalid)?,
        ));
    }
    Ok(compute_readiness(ReadinessInputs {
        dependencies: &dependencies,
        blockers: &blockers,
    }))
}

/// Recheck occurrence custody inside the enqueue/claim/launch write span.
pub(crate) fn guard_occurrence_dispatch(
    conn: &rusqlite::Connection,
    ticket: TicketId,
) -> Result<(), ApiError> {
    let is_template: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM schedules WHERE ticket_id=?1 AND trigger_kind='cron')",
            [ticket.value() as i64],
            |row| row.get(0),
        )
        .map_err(internal)?;
    let mut query = conn
        .prepare(
            "SELECT t.id,t.state FROM task_occurrences current
         JOIN task_occurrences peer ON peer.template_ticket_id=current.template_ticket_id
         JOIN tickets t ON t.id=peer.occurrence_ticket_id
         WHERE current.occurrence_ticket_id=?1 AND peer.occurrence_ticket_id<>?1",
        )
        .map_err(internal)?;
    let rows = query
        .query_map([ticket.value() as i64], |row| {
            Ok((read_id(row, 0)?, row.get::<_, String>(1)?))
        })
        .map_err(internal)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(internal)?;
    let peers = rows
        .into_iter()
        .map(|(id, state)| {
            let state = kanban_domain::TicketState::parse(&state)
                .ok_or_else(|| ApiError::internal("stored occurrence state is invalid"))?;
            Ok((TicketId::new(id), state))
        })
        .collect::<Result<Vec<_>, ApiError>>()?;
    kanban_domain::recurrence::guard_recurring_dispatch(is_template, &peers).map_err(invalid)
}
