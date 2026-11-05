//! SQLite-backed Linear issue fields and history.

use std::cell::RefCell;
use std::collections::HashMap;

use kanban_app::TimelineEnvelope;
use kanban_app::issue_names::{NamedKind, refuse_taken_name};
use kanban_domain::{IssueKey, IssueStateTimestamps, TicketState, WorkflowStateType};
use kanban_dto::{
    ApiError, IssueRelationRecord, TimelineEntityKind, TimelineEntityRef, TimelineEventKind,
};
use rusqlite::{OptionalExtension, params};

use crate::db::{ConnectionHandle, Database, WriteSpan};
use crate::error::StorageError;
use crate::timeline::insert_event;

use super::linear_issue::{
    WorkflowStateRecord, first_workflow_state_of_type, workflow_state_by_legacy,
    workflow_state_by_name,
};
thread_local! {
    static SECONDARY_EFFECTS: RefCell<Option<Vec<kanban_app::SecondaryEffect>>> =
        const { RefCell::new(None) };
}

fn open_secondary_effects() {
    SECONDARY_EFFECTS.with(|slot| *slot.borrow_mut() = Some(Vec::new()));
}

fn drain_secondary_effects() -> Vec<kanban_app::SecondaryEffect> {
    SECONDARY_EFFECTS.with(|slot| slot.borrow_mut().take().unwrap_or_default())
}

/// Open the thread-local record a legacy Ticket write shares with an
/// operator save. A cascade notes each issue it is about to move only
/// while this record is open.
pub(crate) fn begin_secondary_effect_recording() {
    open_secondary_effects();
}

/// Take and clear the record [`begin_secondary_effect_recording`]
/// opened. Empty when nothing was recording.
pub(crate) fn take_secondary_effect_recording() -> Vec<kanban_app::SecondaryEffect> {
    drain_secondary_effects()
}

fn note_secondary_effect(conn: &rusqlite::Connection, ticket_id: i64) {
    let recording = SECONDARY_EFFECTS.with(|slot| slot.borrow().is_some());
    if !recording {
        return;
    }
    let already = SECONDARY_EFFECTS.with(|slot| {
        slot.borrow()
            .as_ref()
            .is_some_and(|rows| rows.iter().any(|row| row.ticket_id == ticket_id))
    });
    if already {
        return;
    }
    let Ok((version, state)) = conn.query_row(
        "SELECT version, state FROM tickets WHERE id = ?1",
        [ticket_id],
        |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
    ) else {
        return;
    };
    let Some(state) = TicketState::parse(&state) else {
        return;
    };
    SECONDARY_EFFECTS.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(rows) = slot.as_mut() else {
            return;
        };
        if rows.iter().any(|row| row.ticket_id == ticket_id) {
            return;
        }
        rows.push(kanban_app::SecondaryEffect {
            ticket_id,
            version: version as u64,
            state,
        });
    });
}

/// A calendar date exactly as Linear stores it: zero-padded
/// `YYYY-MM-DD` with no surrounding whitespace and a real day of a
/// real month, kept verbatim when it passes.
pub(crate) fn validate_calendar_date(raw: &str, field: &'static str) -> Result<String, ApiError> {
    let bytes = raw.as_bytes();
    let padded_shape = bytes.len() == 10
        && bytes[..4].iter().all(u8::is_ascii_digit)
        && bytes[4] == b'-'
        && bytes[5..7].iter().all(u8::is_ascii_digit)
        && bytes[7] == b'-'
        && bytes[8..].iter().all(u8::is_ascii_digit);
    if !padded_shape {
        return Err(ApiError::invalid_request_field(
            field,
            &format!("{field} must be a zero-padded YYYY-MM-DD calendar date"),
        ));
    }
    chrono::NaiveDate::parse_from_str(raw, "%Y-%m-%d").map_err(|_| {
        ApiError::invalid_request_field(field, &format!("{field} is not a real calendar date"))
    })?;
    Ok(raw.to_owned())
}

/// Issue persistence beside the Ticket port.
pub struct SqliteIssueStore {
    conn: ConnectionHandle,
}

impl SqliteIssueStore {
    pub fn new(database: &Database) -> Self {
        Self {
            conn: database.connection_handle(),
        }
    }

    fn lock(&self) -> parking_lot::ReentrantMutexGuard<'_, rusqlite::Connection> {
        self.conn.lock()
    }
}

impl SqliteIssueStore {
    pub fn touch_updated_at(&self, ticket_id: i64) -> Result<(), ApiError> {
        let conn = self.lock();
        conn.execute(
            "UPDATE tickets SET updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') WHERE id = ?1",
            [ticket_id],
        )
        .map_err(internal)?;
        Ok(())
    }

    pub fn set_description(&self, ticket_id: i64, description: &str) -> Result<(), ApiError> {
        let conn = self.lock();
        let current: Option<String> = conn
            .query_row(
                "SELECT description FROM tickets WHERE id = ?1",
                [ticket_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(internal)?;
        if current.as_deref() == Some(description) {
            return Ok(());
        }
        conn.execute(
            "UPDATE tickets SET description = ?2, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'), version = version + 1 WHERE id = ?1",
            params![ticket_id, description],
        )
        .map_err(internal)?;
        drop(conn);
        self.sync_description_mentions(ticket_id, description)?;
        Ok(())
    }

    pub fn patch_description(&self, ticket_id: i64, patch: &str) -> Result<String, ApiError> {
        let conn = self.lock();
        let current: String = conn
            .query_row(
                "SELECT description FROM tickets WHERE id = ?1",
                [ticket_id],
                |row| row.get(0),
            )
            .map_err(not_found_ticket)?;
        let merged = if let Ok(value) = serde_json::from_str::<serde_json::Value>(patch) {
            let find = value.get("find").and_then(|v| v.as_str()).ok_or_else(|| {
                ApiError::invalid_request_field(
                    "description_patch",
                    "description_patch.find is required",
                )
            })?;
            let replace = value
                .get("replace")
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    ApiError::invalid_request_field(
                        "description_patch",
                        "description_patch.replace is required",
                    )
                })?;
            if !current.contains(find) {
                return Err(ApiError::invalid_request_field(
                    "description_patch",
                    "description_patch.find did not match the current description",
                ));
            }
            current.replacen(find, replace, 1)
        } else {
            return Err(ApiError::invalid_request_field(
                "description_patch",
                "description_patch must be a JSON object with find and replace",
            ));
        };
        if merged == current {
            drop(conn);
            return Ok(merged);
        }
        conn.execute(
            "UPDATE tickets SET description = ?2, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'), version = version + 1 WHERE id = ?1",
            params![ticket_id, &merged],
        )
        .map_err(internal)?;
        drop(conn);
        self.sync_description_mentions(ticket_id, &merged)?;
        Ok(merged)
    }

    pub fn set_linear_priority(&self, ticket_id: i64, priority: u8) -> Result<(), ApiError> {
        if priority > 4 {
            return Err(ApiError::invalid_request_field(
                "priority",
                "priority must be between 0 and 4",
            ));
        }
        let conn = self.lock();
        let current: Option<i64> = conn
            .query_row(
                "SELECT linear_priority FROM tickets WHERE id = ?1",
                [ticket_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(internal)?;
        if current == Some(i64::from(priority)) {
            return Ok(());
        }
        // The issue model's priority is the operator-facing value,
        // but the board and the legacy Ticket row keep their own
        // column: every issue-model priority change carries the
        // legacy priority with it, so the board's filters and order
        // follow. No priority (0) maps to the legacy normal bucket,
        // the vocabulary's default, so the card neither drops off
        // the board nor changes buckets spuriously.
        conn.execute(
            "UPDATE tickets
             SET linear_priority = ?2,
                 priority = ?3,
                 updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
                 version = version + 1
             WHERE id = ?1",
            params![
                ticket_id,
                i64::from(priority),
                kanban_domain::Priority::of_linear(priority).wire_name(),
            ],
        )
        .map_err(internal)?;
        Ok(())
    }

    /// Record a new issue's default Linear priority. Part of creation,
    /// not an edit: unlike [`SqliteIssueStore::set_linear_priority`]
    /// this never bumps the version, because the caller sent no
    /// priority to change.
    pub fn set_initial_linear_priority(
        &self,
        ticket_id: i64,
        priority: u8,
    ) -> Result<(), ApiError> {
        let conn = self.lock();
        conn.execute(
            "UPDATE tickets SET linear_priority = ?2 WHERE id = ?1",
            params![ticket_id, i64::from(priority)],
        )
        .map_err(internal)?;
        Ok(())
    }

    pub fn set_due_date(&self, ticket_id: i64, due_date: Option<&str>) -> Result<(), ApiError> {
        let conn = self.lock();
        // No due date is the empty none value, never a calendar date:
        // every valid date, however far out, round-trips.
        let stored = match due_date {
            None => String::new(),
            Some(raw) => validate_calendar_date(raw, "due_date")?,
        };
        let current: Option<String> = conn
            .query_row(
                "SELECT due_date FROM tickets WHERE id = ?1",
                [ticket_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(internal)?;
        if current.as_deref() == Some(stored.as_str()) {
            return Ok(());
        }
        let changed = conn
            .execute(
                "UPDATE tickets SET due_date = ?2, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'), version = version + 1 WHERE id = ?1",
                params![ticket_id, stored],
            )
            .map_err(internal)?;
        if changed == 0 {
            return Err(not_found_ticket(rusqlite::Error::QueryReturnedNoRows));
        }
        Ok(())
    }

    pub fn set_milestone(&self, ticket_id: i64, milestone_id: Option<i64>) -> Result<(), ApiError> {
        let conn = self.lock();
        if let Some(milestone_id) = milestone_id {
            let milestone_project: Option<i64> = conn
                .query_row(
                    "SELECT project_id FROM issue_milestones WHERE id = ?1",
                    [milestone_id],
                    |row| row.get(0),
                )
                .optional()
                .map_err(internal)?;
            let Some(milestone_project) = milestone_project else {
                return Err(ApiError::invalid_request_field(
                    "milestone_id",
                    "unknown milestone",
                ));
            };
            let ticket_project: i64 = conn
                .query_row(
                    "SELECT project_id FROM tickets WHERE id = ?1",
                    [ticket_id],
                    |row| row.get(0),
                )
                .map_err(not_found_ticket)?;
            if milestone_project != ticket_project {
                return Err(ApiError::invalid_request_field(
                    "milestone_id",
                    "milestone belongs to a different project",
                ));
            }
        }
        let current: Option<Option<i64>> = conn
            .query_row(
                "SELECT milestone_id FROM tickets WHERE id = ?1",
                [ticket_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(internal)?;
        if current.flatten() == milestone_id {
            return Ok(());
        }
        conn.execute(
            "UPDATE tickets SET milestone_id = ?2, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'), version = version + 1 WHERE id = ?1",
            params![ticket_id, milestone_id],
        )
        .map_err(internal)?;
        Ok(())
    }

    pub fn record_initial_state(&self, ticket_id: i64, state_name: &str) -> Result<(), ApiError> {
        let conn = self.lock();
        let span = WriteSpan::begin(&conn).map_err(internal)?;
        let (project_id, created_at, prior_workflow): (i64, String, Option<i64>) = span
            .query_row(
                "SELECT project_id, created_at, workflow_state_id FROM tickets WHERE id = ?1",
                [ticket_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .map_err(not_found_ticket)?;
        let state = resolve_state_by_name(&span, project_id, state_name)?;
        if prior_workflow == Some(state.id) {
            span.commit().map_err(internal)?;
            return Ok(());
        }
        // The Ticket row's creation seeds the initial Draft state in
        // the same write, so a requested first state replaces that
        // seed: creation owns the whole history, and no state before
        // it exists to close. Once anything else has moved the issue,
        // the first recorded state stands.
        let seeded_rows: i64 = span
            .query_row(
                "SELECT COUNT(*) FROM issue_state_history
                 WHERE ticket_id = ?1 AND started_at = ?2",
                params![ticket_id, &created_at],
                |row| row.get(0),
            )
            .map_err(internal)?;
        if seeded_rows != 1 {
            span.commit().map_err(internal)?;
            return Ok(());
        }
        span.execute(
            "DELETE FROM issue_state_history WHERE ticket_id = ?1",
            [ticket_id],
        )
        .map_err(internal)?;
        let legacy = TicketState::parse(&state.legacy_state)
            .ok_or_else(|| ApiError::internal("stored workflow state is invalid"))?;
        let mut stamps = IssueStateTimestamps::default();
        stamps.stamp_entry(state.state_type, &created_at);
        stamps.clear_terminal_on_reopen(state.state_type);
        span.execute(
            "INSERT INTO issue_state_history (ticket_id, workflow_state_id, started_at, ended_at)
             VALUES (?1, ?2, ?3, NULL)",
            params![ticket_id, state.id, &created_at],
        )
        .map_err(internal)?;
        span.execute(
            "UPDATE tickets
             SET state = ?2,
                 workflow_state_id = ?3,
                 started_at = ?4,
                 completed_at = ?5,
                 canceled_at = ?6
             WHERE id = ?1",
            params![
                ticket_id,
                legacy.wire_name(),
                state.id,
                stamps.started_at,
                stamps.completed_at,
                stamps.canceled_at,
            ],
        )
        .map_err(internal)?;
        let parent_id: Option<i64> = span
            .query_row(
                "SELECT parent_ticket_id FROM tickets WHERE id = ?1",
                [ticket_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(internal)?
            .flatten();
        span.commit().map_err(internal)?;
        if (state.state_type == WorkflowStateType::Completed
            || state.state_type == WorkflowStateType::Canceled
            || state.state_type == WorkflowStateType::Duplicate)
            && let Some(parent_id) = parent_id
        {
            self.try_auto_close_parent_when_children_done(parent_id)?;
        }
        Ok(())
    }

    pub fn apply_free_state(
        &self,
        ticket_id: i64,
        project_id: i64,
        to_state: &WorkflowStateRecord,
        envelope: TimelineEnvelope,
    ) -> Result<(), ApiError> {
        let conn = self.lock();
        apply_free_state_on(&conn, ticket_id, project_id, to_state, envelope)
    }

    fn try_auto_close_parent_when_children_done(&self, parent_id: i64) -> Result<(), ApiError> {
        let conn = self.lock();
        auto_close_parent_if_children_done(&conn, parent_id)
    }

    fn parent_would_cycle(&self, ticket_id: i64, parent_id: i64) -> Result<bool, ApiError> {
        let conn = self.lock();
        let mut current = Some(parent_id);
        while let Some(id) = current {
            if id == ticket_id {
                return Ok(true);
            }
            current = conn
                .query_row(
                    "SELECT parent_ticket_id FROM tickets WHERE id = ?1",
                    [id],
                    |row| row.get(0),
                )
                .optional()
                .map_err(internal)?
                .flatten();
        }
        Ok(false)
    }

    pub fn list_labels(&self, ticket_id: i64) -> Result<Vec<String>, ApiError> {
        let conn = self.lock();
        let mut statement = conn
            .prepare(
                "SELECT l.name FROM issue_ticket_labels AS tl
                 JOIN issue_labels AS l ON l.id = tl.label_id
                 WHERE tl.ticket_id = ?1
                 ORDER BY l.name",
            )
            .map_err(internal)?;
        let rows = statement
            .query_map([ticket_id], |row| row.get(0))
            .map_err(internal)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(internal)
    }

    pub fn add_label(&self, project_id: i64, ticket_id: i64, name: &str) -> Result<(), ApiError> {
        let conn = self.lock();
        let row: Option<(i64, Option<String>)> = conn
            .query_row(
                "SELECT id, retired_at FROM issue_labels WHERE project_id = ?1 AND name = ?2",
                params![project_id, name],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(internal)?;
        let Some((label_id, retired_at)) = row else {
            return Err(ApiError::invalid_request_field(
                "add_labels",
                &format!("unknown label `{name}`"),
            ));
        };
        if retired_at.is_some() {
            return Err(ApiError::invalid_request_field(
                "add_labels",
                &format!("label `{name}` is retired"),
            ));
        };
        let group: Option<(i64, i64)> = conn
            .query_row(
                "SELECT g.id, g.single_select FROM issue_labels AS l
                 JOIN issue_label_groups AS g ON g.id = l.group_id
                 WHERE l.id = ?1",
                [label_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(internal)?;
        // A save that changes nothing must touch nothing: an add of a
        // label the issue already holds — and that displaces no other
        // member of its single-select group — leaves the update time
        // and the version alone.
        let held: Option<i64> = conn
            .query_row(
                "SELECT label_id FROM issue_ticket_labels
                 WHERE ticket_id = ?1 AND label_id = ?2",
                params![ticket_id, label_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(internal)?;
        let mut changed = held.is_none();
        if let Some((group_id, single)) = group
            && single == 1
        {
            let displaced = conn
                .execute(
                    "DELETE FROM issue_ticket_labels
                     WHERE ticket_id = ?1
                       AND label_id <> ?3
                       AND label_id IN (
                           SELECT l.id FROM issue_labels AS l
                           WHERE l.group_id = ?2
                       )",
                    params![ticket_id, group_id, label_id],
                )
                .map_err(internal)?;
            changed = changed || displaced > 0;
        }
        conn.execute(
            "INSERT OR IGNORE INTO issue_ticket_labels (ticket_id, label_id) VALUES (?1, ?2)",
            params![ticket_id, label_id],
        )
        .map_err(internal)?;
        if changed {
            drop(conn);
            self.bump_version(ticket_id)?;
        }
        Ok(())
    }

    pub fn remove_label(
        &self,
        project_id: i64,
        ticket_id: i64,
        name: &str,
    ) -> Result<(), ApiError> {
        let conn = self.lock();
        let label_id: Option<i64> = conn
            .query_row(
                "SELECT id FROM issue_labels WHERE project_id = ?1 AND name = ?2",
                params![project_id, name],
                |row| row.get(0),
            )
            .optional()
            .map_err(internal)?;
        let Some(label_id) = label_id else {
            return Err(ApiError::not_found_field(
                "remove_labels",
                &format!("unknown label `{name}`"),
            ));
        };
        let changed = conn
            .execute(
                "DELETE FROM issue_ticket_labels WHERE ticket_id = ?1 AND label_id = ?2",
                params![ticket_id, label_id],
            )
            .map_err(internal)?;
        if changed == 0 {
            return Ok(());
        }
        drop(conn);
        self.bump_version(ticket_id)?;
        Ok(())
    }

    pub fn load_issue_key(&self, ticket_id: i64) -> Result<IssueKey, ApiError> {
        let conn = self.lock();
        let (code, number): (String, i64) = conn
            .query_row(
                "SELECT p.code, t.number FROM tickets AS t
                 JOIN projects AS p ON p.id = t.project_id
                 WHERE t.id = ?1",
                [ticket_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(not_found_ticket)?;
        let code = kanban_domain::ProjectCode::new(&code)
            .map_err(|_| ApiError::internal("project code is invalid"))?;
        let number = kanban_domain::TicketNumber::new(number as u64)
            .map_err(|_| ApiError::internal("ticket number is invalid"))?;
        Ok(IssueKey::new(code, number))
    }

    pub fn workflow_state_for_ticket(
        &self,
        ticket_id: i64,
    ) -> Result<Option<WorkflowStateRecord>, ApiError> {
        let conn = self.lock();
        let project_id: i64 = conn
            .query_row(
                "SELECT project_id FROM tickets WHERE id = ?1",
                [ticket_id],
                |row| row.get(0),
            )
            .map_err(not_found_ticket)?;
        let state_id: Option<i64> = conn
            .query_row(
                "SELECT workflow_state_id FROM tickets WHERE id = ?1",
                [ticket_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(internal)?
            .flatten();
        if let Some(id) = state_id {
            let mut statement = conn
                .prepare(
                    "SELECT id, project_id, name, type, legacy_state, position
                     FROM issue_workflow_states WHERE id = ?1",
                )
                .map_err(internal)?;
            return statement
                .query_row([id], WorkflowStateRecord::map_row)
                .optional()
                .map_err(internal)
                .map_err(|_| ApiError::internal("workflow state is missing"));
        }
        let legacy: String = conn
            .query_row(
                "SELECT state FROM tickets WHERE id = ?1",
                [ticket_id],
                |row| row.get(0),
            )
            .map_err(not_found_ticket)?;
        workflow_state_by_legacy(&conn, project_id, &legacy).map_err(internal_storage)
    }

    pub fn connection(&self) -> parking_lot::ReentrantMutexGuard<'_, rusqlite::Connection> {
        self.conn.lock()
    }

    pub fn resolve_identifier(
        &self,
        identifier: &str,
    ) -> Result<Option<(IssueKey, i64)>, ApiError> {
        let conn = self.lock();
        super::linear_issue::resolve_issue_key(&conn, identifier).map_err(internal_storage)
    }

    pub fn list_workflow_states_for_project(
        &self,
        project_id: i64,
    ) -> Result<Vec<WorkflowStateRecord>, ApiError> {
        let conn = self.lock();
        super::linear_issue::list_workflow_states(&conn, project_id).map_err(internal_storage)
    }

    pub fn list_project_labels(
        &self,
        project_id: i64,
    ) -> Result<Vec<kanban_app::IssueLabelRow>, ApiError> {
        let conn = self.lock();
        let mut statement = conn
            .prepare(
                "SELECT l.name, l.color, g.name, COALESCE(g.single_select, 0), l.retired_at IS NOT NULL
                 FROM issue_labels AS l
                 LEFT JOIN issue_label_groups AS g ON g.id = l.group_id
                 WHERE l.project_id = ?1
                 ORDER BY l.name",
            )
            .map_err(internal)?;
        statement
            .query_map([project_id], |row| {
                Ok(kanban_app::IssueLabelRow {
                    name: row.get(0)?,
                    color: row.get(1)?,
                    group: row.get(2)?,
                    group_single_select: row.get::<_, i64>(3)? != 0,
                    retired: row.get(4)?,
                })
            })
            .map_err(internal)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(internal)
    }

    pub fn list_label_groups(&self, project_id: i64) -> Result<Vec<String>, ApiError> {
        let conn = self.lock();
        let mut statement = conn
            .prepare("SELECT name FROM issue_label_groups WHERE project_id = ?1 ORDER BY name")
            .map_err(internal)?;
        statement
            .query_map([project_id], |row| row.get(0))
            .map_err(internal)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(internal)
    }

    pub fn list_project_milestones(
        &self,
        project_id: i64,
    ) -> Result<Vec<kanban_app::IssueMilestoneRow>, ApiError> {
        let conn = self.lock();
        let mut statement = conn
            .prepare(
                "SELECT m.id, m.name, m.target_date,
                        (SELECT COUNT(*) FROM tickets AS t WHERE t.milestone_id = m.id),
                        (SELECT COUNT(*) FROM tickets AS t
                         JOIN issue_workflow_states AS s ON s.id = t.workflow_state_id
                         WHERE t.milestone_id = m.id
                           AND s.type IN ('completed', 'canceled', 'duplicate'))
                 FROM issue_milestones AS m
                 WHERE m.project_id = ?1 ORDER BY m.sort_order, m.name",
            )
            .map_err(internal)?;
        statement
            .query_map([project_id], |row| {
                Ok(kanban_app::IssueMilestoneRow {
                    id: row.get::<_, i64>(0)? as u64,
                    name: row.get(1)?,
                    target_date: row.get(2)?,
                    issues_total: row.get::<_, i64>(3)? as u32,
                    issues_done: row.get::<_, i64>(4)? as u32,
                })
            })
            .map_err(internal)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(internal)
    }

    pub fn resolve_state_by_name(
        &self,
        project_id: i64,
        name: &str,
    ) -> Result<WorkflowStateRecord, ApiError> {
        let conn = self.lock();
        resolve_state_by_name(&conn, project_id, name)
    }

    pub fn issue_fields(&self, ticket_id: i64) -> Result<kanban_app::IssueTicketFields, ApiError> {
        let conn = self.lock();
        conn.query_row(
            "SELECT description, linear_priority, due_date, milestone_id, updated_at, created_at, version, parent_ticket_id, duplicate_of_ticket_id, started_at, completed_at, canceled_at, assignee, creator
             FROM tickets WHERE id = ?1",
            [ticket_id],
            |row| {
                Ok(kanban_app::IssueTicketFields {
                    description: row.get(0)?,
                    linear_priority: row.get::<_, i64>(1)? as u8,
                    due_date: {
                        let stored: String = row.get(2)?;
                        (!stored.is_empty()).then_some(stored)
                    },
                    milestone_id: row.get(3)?,
                    updated_at: row.get(4)?,
                    created_at: row.get(5)?,
                    version: row.get::<_, i64>(6)? as u64,
                    parent_ticket_id: row.get(7)?,
                    duplicate_of_ticket_id: row.get(8)?,
                    started_at: row.get(9)?,
                    completed_at: row.get(10)?,
                    canceled_at: row.get(11)?,
                    assignee: row.get(12)?,
                    creator: row.get(13)?,
                })
            },
        )
        .map_err(not_found_ticket)
    }

    pub fn list_state_history(
        &self,
        ticket_id: i64,
    ) -> Result<Vec<kanban_dto::IssueStateHistoryRecord>, ApiError> {
        let conn = self.lock();
        let mut statement = conn
            .prepare(
                "SELECT s.name, s.type, h.started_at, h.ended_at
                 FROM issue_state_history AS h
                 JOIN issue_workflow_states AS s ON s.id = h.workflow_state_id
                 WHERE h.ticket_id = ?1
                 ORDER BY h.started_at ASC, h.id ASC",
            )
            .map_err(internal)?;
        statement
            .query_map([ticket_id], |row| {
                Ok(kanban_dto::IssueStateHistoryRecord {
                    state: row.get(0)?,
                    state_type: row.get(1)?,
                    started_at: row.get(2)?,
                    ended_at: row.get(3)?,
                })
            })
            .map_err(internal)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(internal)
    }

    pub fn set_title(&self, ticket_id: i64, title: &str) -> Result<(), ApiError> {
        let conn = self.lock();
        let (kind, current): (String, Option<String>) = conn
            .query_row(
                "SELECT kind, CASE kind WHEN 'implementation' THEN slice ELSE title END
                 FROM tickets WHERE id = ?1",
                [ticket_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(not_found_ticket)?;
        if current.as_deref() == Some(title) {
            return Ok(());
        }
        let changed = if kind == "implementation" {
            conn.execute(
                "UPDATE tickets SET slice = ?2, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'), version = version + 1
                 WHERE id = ?1",
                params![ticket_id, title],
            )
            .map_err(internal)?
        } else {
            conn.execute(
                "UPDATE tickets SET title = ?2, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'), version = version + 1
                 WHERE id = ?1",
                params![ticket_id, title],
            )
            .map_err(internal)?
        };
        if changed == 0 {
            return Err(not_found_ticket(rusqlite::Error::QueryReturnedNoRows));
        }
        Ok(())
    }

    pub fn bump_version(&self, ticket_id: i64) -> Result<u64, ApiError> {
        let conn = self.lock();
        note_secondary_effect(&conn, ticket_id);
        conn.execute(
            "UPDATE tickets SET version = version + 1, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
             WHERE id = ?1",
            [ticket_id],
        )
        .map_err(internal)?;
        let version: i64 = conn
            .query_row(
                "SELECT version FROM tickets WHERE id = ?1",
                [ticket_id],
                |row| row.get(0),
            )
            .map_err(not_found_ticket)?;
        Ok(version as u64)
    }

    pub fn set_parent(&self, ticket_id: i64, parent_id: Option<i64>) -> Result<(), ApiError> {
        if parent_id == Some(ticket_id) {
            return Err(ApiError::invalid_request_field(
                "parent_id",
                "an issue cannot be its own parent",
            ));
        }
        if let Some(parent_id) = parent_id {
            if self.parent_would_cycle(ticket_id, parent_id)? {
                return Err(ApiError::invalid_request_field(
                    "parent_id",
                    "setting this parent would create a cycle",
                ));
            }
            let conn = self.lock();
            let exists: Option<i64> = conn
                .query_row("SELECT id FROM tickets WHERE id = ?1", [parent_id], |row| {
                    row.get(0)
                })
                .optional()
                .map_err(internal)?;
            if exists.is_none() {
                return Err(ApiError::not_found_field("parent_id", "parent issue"));
            }
            // As in Linear, the parent may sit in another project; the
            // child keeps its own project and milestone.
            drop(conn);
        }
        let conn = self.lock();
        let current: Option<Option<i64>> = conn
            .query_row(
                "SELECT parent_ticket_id FROM tickets WHERE id = ?1",
                [ticket_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(internal)?;
        let previous = current.flatten();
        if previous == parent_id {
            return Ok(());
        }
        conn.execute(
            "UPDATE tickets SET parent_ticket_id = ?2, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'), version = version + 1
             WHERE id = ?1",
            params![ticket_id, parent_id],
        )
        .map_err(internal)?;
        drop(conn);
        if let Some(old_parent) = previous {
            self.bump_version(old_parent)?;
        }
        if let Some(new_parent) = parent_id {
            self.bump_version(new_parent)?;
        }
        if let Some(old_parent) = previous {
            auto_close_parent_if_children_done(&self.lock(), old_parent)?;
        }
        if let Some(new_parent) = parent_id {
            auto_close_parent_if_children_done(&self.lock(), new_parent)?;
        }
        Ok(())
    }

    pub fn list_children(
        &self,
        ticket_id: i64,
    ) -> Result<Vec<kanban_dto::IssueChildRecord>, ApiError> {
        let conn = self.lock();
        let mut statement = conn
            .prepare(
                "SELECT p.code, t.number, t.title, t.slice, t.kind, s.name, t.sub_issue_sort
                 FROM tickets AS t
                 JOIN projects AS p ON p.id = t.project_id
                 LEFT JOIN issue_workflow_states AS s ON s.id = t.workflow_state_id
                 WHERE t.parent_ticket_id = ?1
                 ORDER BY t.sub_issue_sort, t.number",
            )
            .map_err(internal)?;
        statement
            .query_map([ticket_id], |row| {
                let code: String = row.get(0)?;
                let number: i64 = row.get(1)?;
                let title: Option<String> = row.get(2)?;
                let slice: Option<String> = row.get(3)?;
                let kind: String = row.get(4)?;
                let state: Option<String> = row.get(5)?;
                let sort: f64 = row.get(6)?;
                let title = if kind == "implementation" {
                    slice.unwrap_or_default()
                } else {
                    title.unwrap_or_default()
                };
                Ok(kanban_dto::IssueChildRecord {
                    identifier: format!("{code}-{number}"),
                    title,
                    state: state.unwrap_or_else(|| "Draft".to_owned()),
                    sort_order: sort,
                })
            })
            .map_err(internal)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(internal)
    }

    pub fn add_relation(
        &self,
        from_ticket_id: i64,
        to_identifier: &str,
        kind: &str,
    ) -> Result<(), ApiError> {
        let conn = self.lock();
        let Some(to_ticket_id) =
            resolve_relation_target(&conn, to_identifier).map_err(internal_storage)?
        else {
            return Err(ApiError::not_found(to_identifier));
        };
        if !matches!(kind, "blocks" | "related" | "duplicate") {
            return Err(ApiError::invalid_request(&format!(
                "unsupported relation kind `{kind}`"
            )));
        }
        drop(conn);
        if kind == "blocks" {
            return self.add_blocks_edge(from_ticket_id, to_ticket_id);
        }
        if kind == "related" {
            return self.add_related_edge(from_ticket_id, to_ticket_id);
        }
        let conn = self.lock();
        let changed = conn
            .execute(
                "INSERT OR IGNORE INTO issue_relations (from_ticket_id, to_ticket_id, kind)
                 VALUES (?1, ?2, ?3)",
                params![from_ticket_id, to_ticket_id, kind],
            )
            .map_err(internal)?;
        if changed == 0 {
            return Ok(());
        }
        self.bump_version(from_ticket_id)?;
        Ok(())
    }

    /// One blocks edge: the dependency graph the board and dispatch
    /// readiness read, under the same cycle rule the legacy graph
    /// enforces, moving both issues' versions with the edge.
    fn add_blocks_edge(&self, from: i64, to: i64) -> Result<(), ApiError> {
        let conn = self.lock();
        let mut graph = kanban_domain::TicketDependencyGraph::new();
        let mut statement = conn
            .prepare("SELECT from_ticket, to_ticket FROM ticket_dependencies ORDER BY id")
            .map_err(internal)?;
        let edges = statement
            .query_map([], |row| {
                Ok(kanban_domain::TicketDependency::new(
                    kanban_domain::TicketId::new(row.get::<_, i64>(0)?.unsigned_abs()),
                    kanban_domain::TicketId::new(row.get::<_, i64>(1)?.unsigned_abs()),
                ))
            })
            .map_err(internal)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(internal)?;
        drop(statement);
        for edge in &edges {
            graph
                .add(edge.from(), edge.to())
                .map_err(|error| ApiError::internal(&error.to_string()))?;
        }
        let from_id = kanban_domain::TicketId::new(from.unsigned_abs());
        let to_id = kanban_domain::TicketId::new(to.unsigned_abs());
        if edges
            .iter()
            .any(|edge| edge.from() == from_id && edge.to() == to_id)
        {
            return Ok(());
        }
        graph
            .add(from_id, to_id)
            .map_err(|error| ApiError::invalid_request(&error.to_string()))?;
        let changed = conn
            .execute(
                "INSERT OR IGNORE INTO ticket_dependencies (from_ticket, to_ticket)
                 VALUES (?1, ?2)",
                params![from, to],
            )
            .map_err(internal)?;
        if changed == 0 {
            return Ok(());
        }
        drop(conn);
        self.bump_version(from)?;
        self.bump_version(to)?;
        Ok(())
    }

    /// One related link: undirected, stored in the canonical
    /// min-to-max direction so both endpoints list the same single
    /// row and either endpoint can remove it.
    fn add_related_edge(&self, from: i64, to: i64) -> Result<(), ApiError> {
        let (low, high) = if from < to { (from, to) } else { (to, from) };
        let conn = self.lock();
        let existing: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM issue_relations
                 WHERE kind = 'related'
                   AND ((from_ticket_id = ?1 AND to_ticket_id = ?2)
                     OR (from_ticket_id = ?2 AND to_ticket_id = ?1))",
                params![from, to],
                |row| row.get(0),
            )
            .map_err(internal)?;
        if existing > 0 {
            return Ok(());
        }
        let changed = conn
            .execute(
                "INSERT OR IGNORE INTO issue_relations (from_ticket_id, to_ticket_id, kind)
                 VALUES (?1, ?2, 'related')",
                params![low, high],
            )
            .map_err(internal)?;
        if changed == 0 {
            return Ok(());
        }
        drop(conn);
        self.bump_version(from)?;
        self.bump_version(to)?;
        Ok(())
    }

    pub fn remove_relation(
        &self,
        from_ticket_id: i64,
        to_identifier: &str,
        kind: &str,
    ) -> Result<(), ApiError> {
        let conn = self.lock();
        let Some(to_ticket_id) =
            resolve_relation_target(&conn, to_identifier).map_err(internal_storage)?
        else {
            return Err(ApiError::not_found(to_identifier));
        };
        drop(conn);
        let changed = match kind {
            "blocks" => {
                let conn = self.lock();
                conn.execute(
                    "DELETE FROM ticket_dependencies
                     WHERE from_ticket = ?1 AND to_ticket = ?2",
                    params![from_ticket_id, to_ticket_id],
                )
                .map_err(internal)?
            }
            "related" => {
                let conn = self.lock();
                conn.execute(
                    "DELETE FROM issue_relations
                     WHERE kind = 'related'
                       AND ((from_ticket_id = ?1 AND to_ticket_id = ?2)
                        OR (from_ticket_id = ?2 AND to_ticket_id = ?1))",
                    params![from_ticket_id, to_ticket_id],
                )
                .map_err(internal)?
            }
            _ => {
                let conn = self.lock();
                conn.execute(
                    "DELETE FROM issue_relations
                     WHERE from_ticket_id = ?1 AND to_ticket_id = ?2 AND kind = ?3",
                    params![from_ticket_id, to_ticket_id, kind],
                )
                .map_err(internal)?
            }
        };
        if changed == 0 {
            return Ok(());
        }
        self.bump_version(from_ticket_id)?;
        self.bump_version(to_ticket_id)?;
        Ok(())
    }

    pub fn set_duplicate_of(
        &self,
        ticket_id: i64,
        target_identifier: Option<&str>,
    ) -> Result<(), ApiError> {
        let conn = self.lock();
        let target_id = if let Some(identifier) = target_identifier {
            let Some((_, id)) = super::linear_issue::resolve_issue_key(&conn, identifier)
                .map_err(internal_storage)?
            else {
                return Err(ApiError::not_found_field("duplicate_of", identifier));
            };
            if id == ticket_id {
                return Err(ApiError::invalid_request_field(
                    "duplicate_of",
                    "an issue cannot be a duplicate of itself",
                ));
            }
            Some(id)
        } else {
            None
        };
        let (project_id, current_target): (i64, Option<i64>) = conn
            .query_row(
                "SELECT project_id, duplicate_of_ticket_id FROM tickets WHERE id = ?1",
                [ticket_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(not_found_ticket)?;
        drop(conn);

        if current_target == target_id
            && (target_id.is_none() || self.sits_in_duplicate_state(ticket_id)?)
        {
            return Ok(());
        }
        let mut target_relations_changed = false;
        if let Some(target_id) = target_id {
            target_relations_changed =
                self.relink_relations_for_duplicate_merge(ticket_id, target_id)?;
            let conn = self.lock();
            conn.execute(
                "UPDATE tickets SET duplicate_of_ticket_id = ?2,
                 updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'), version = version + 1
                 WHERE id = ?1",
                params![ticket_id, target_id],
            )
            .map_err(internal)?;
            drop(conn);
            let duplicate_state =
                super::linear_issue::workflow_state_by_name(&self.lock(), project_id, "Duplicate")
                    .map_err(internal_storage)?
                    .ok_or_else(|| ApiError::internal("Duplicate workflow state missing"))?;
            let envelope = TimelineEnvelope::project(
                project_id as u64,
                kanban_dto::TimelineEventKind::Transition,
                Some(TimelineEntityRef {
                    kind: TimelineEntityKind::Ticket,
                    id: ticket_id.to_string(),
                }),
                serde_json::json!({
                    "action": "marked_duplicate",
                    "duplicate_of": target_id,
                }),
            );
            self.apply_free_state(ticket_id, project_id, &duplicate_state, envelope)?;
        } else {
            let conn = self.lock();
            conn.execute(
                "UPDATE tickets SET duplicate_of_ticket_id = NULL,
                 updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'), version = version + 1
                 WHERE id = ?1",
                [ticket_id],
            )
            .map_err(internal)?;
        }
        if current_target != target_id {
            if let Some(previous) = current_target {
                self.bump_version(previous)?;
            }
            if let Some(next) = target_id {
                self.bump_version(next)?;
            }
        } else if target_relations_changed && let Some(next) = target_id {
            self.bump_version(next)?;
        }
        Ok(())
    }

    fn sits_in_duplicate_state(&self, ticket_id: i64) -> Result<bool, ApiError> {
        Ok(self
            .workflow_state_for_ticket(ticket_id)?
            .is_some_and(|state| state.name == "Duplicate"))
    }

    fn relink_relations_for_duplicate_merge(
        &self,
        ticket_id: i64,
        target_id: i64,
    ) -> Result<bool, ApiError> {
        let conn = self.lock();
        let mut relation_query = conn
            .prepare(
                "SELECT from_ticket_id, to_ticket_id, kind
                 FROM issue_relations
                 WHERE from_ticket_id = ?1 OR to_ticket_id = ?1",
            )
            .map_err(internal)?;
        let relations = relation_query
            .query_map([ticket_id], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })
            .map_err(internal)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(internal)?;
        drop(relation_query);
        let mut dependency_query = conn
            .prepare(
                "SELECT from_ticket, to_ticket
                 FROM ticket_dependencies
                 WHERE from_ticket = ?1 OR to_ticket = ?1",
            )
            .map_err(internal)?;
        let dependencies = dependency_query
            .query_map([ticket_id], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
            })
            .map_err(internal)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(internal)?;
        drop(dependency_query);
        conn.execute(
            "DELETE FROM issue_relations
             WHERE from_ticket_id = ?1 OR to_ticket_id = ?1",
            [ticket_id],
        )
        .map_err(internal)?;
        conn.execute(
            "DELETE FROM ticket_dependencies
             WHERE from_ticket = ?1 OR to_ticket = ?1",
            [ticket_id],
        )
        .map_err(internal)?;
        let mut affected = std::collections::BTreeSet::new();
        for (from, to, kind) in relations {
            let other = if from == ticket_id { to } else { from };
            if other == ticket_id {
                continue;
            }
            affected.insert(other);
            if other == target_id {
                affected.insert(target_id);
                continue;
            }
            let inserted = if kind == "related" {
                let (low, high) = if target_id < other {
                    (target_id, other)
                } else {
                    (other, target_id)
                };
                conn.execute(
                    "INSERT OR IGNORE INTO issue_relations (from_ticket_id, to_ticket_id, kind)
                     VALUES (?1, ?2, 'related')",
                    params![low, high],
                )
                .map_err(internal)?
            } else {
                let (new_from, new_to) = if from == ticket_id {
                    (target_id, other)
                } else {
                    (other, target_id)
                };
                if new_from == new_to {
                    0
                } else {
                    conn.execute(
                        "INSERT OR IGNORE INTO issue_relations (from_ticket_id, to_ticket_id, kind)
                         VALUES (?1, ?2, ?3)",
                        params![new_from, new_to, kind],
                    )
                    .map_err(internal)?
                }
            };
            if inserted > 0 {
                affected.insert(target_id);
            }
        }
        let remaining = {
            let mut remaining_query = conn
                .prepare("SELECT from_ticket, to_ticket FROM ticket_dependencies")
                .map_err(internal)?;
            remaining_query
                .query_map([], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)))
                .map_err(internal)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(internal)?
        };
        let mut graph = kanban_domain::TicketDependencyGraph::restore(
            remaining
                .into_iter()
                .map(|(from, to)| {
                    kanban_domain::TicketDependency::new(
                        kanban_domain::TicketId::new(from.unsigned_abs()),
                        kanban_domain::TicketId::new(to.unsigned_abs()),
                    )
                })
                .collect(),
        );
        for (from, to) in dependencies {
            let other = if from == ticket_id { to } else { from };
            if other == ticket_id {
                continue;
            }
            affected.insert(other);
            if other == target_id {
                affected.insert(target_id);
                continue;
            }
            let (new_from, new_to) = if from == ticket_id {
                (target_id, other)
            } else {
                (other, target_id)
            };
            if new_from == new_to {
                continue;
            }
            let from_id = kanban_domain::TicketId::new(new_from.unsigned_abs());
            let to_id = kanban_domain::TicketId::new(new_to.unsigned_abs());
            match graph.add(from_id, to_id) {
                Ok(()) => {}
                Err(kanban_domain::DependencyError::Cycle { .. })
                | Err(kanban_domain::DependencyError::DuplicateEdge)
                | Err(kanban_domain::DependencyError::SelfEdge) => continue,
                Err(error) => return Err(ApiError::internal(&error.to_string())),
            }
            conn.execute(
                "INSERT OR IGNORE INTO ticket_dependencies (from_ticket, to_ticket)
                 VALUES (?1, ?2)",
                params![new_from, new_to],
            )
            .map_err(internal)?;
            affected.insert(target_id);
        }
        drop(conn);
        let target_relations_changed = affected.contains(&target_id);
        affected.remove(&ticket_id);
        affected.remove(&target_id);
        for id in affected {
            self.bump_version(id)?;
        }
        Ok(target_relations_changed)
    }

    pub fn list_duplicate_identifiers(&self, ticket_id: i64) -> Result<Vec<String>, ApiError> {
        let conn = self.lock();
        let mut statement = conn
            .prepare(
                "SELECT p.code, t.number
                 FROM tickets AS t
                 JOIN projects AS p ON p.id = t.project_id
                 WHERE t.duplicate_of_ticket_id = ?1
                 ORDER BY t.number",
            )
            .map_err(internal)?;
        statement
            .query_map([ticket_id], |row| {
                let code: String = row.get(0)?;
                let number: i64 = row.get(1)?;
                Ok(format!("{code}-{number}"))
            })
            .map_err(internal)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(internal)
    }

    pub fn set_sub_issue_sort(&self, ticket_id: i64, sort: f64) -> Result<(), ApiError> {
        let conn = self.lock();
        let current: Option<f64> = conn
            .query_row(
                "SELECT sub_issue_sort FROM tickets WHERE id = ?1",
                [ticket_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(internal)?;
        if current == Some(sort) {
            return Ok(());
        }
        conn.execute(
            "UPDATE tickets SET sub_issue_sort = ?2,
             updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'), version = version + 1
             WHERE id = ?1",
            params![ticket_id, sort],
        )
        .map_err(internal)?;
        Ok(())
    }

    pub fn append_sub_issue_sort(&self, ticket_id: i64, parent_id: i64) -> Result<(), ApiError> {
        let conn = self.lock();
        let next: f64 = conn
            .query_row(
                "SELECT COALESCE(MAX(sub_issue_sort), -1) + 1
                 FROM tickets
                 WHERE parent_ticket_id = ?1 AND id <> ?2",
                params![parent_id, ticket_id],
                |row| row.get(0),
            )
            .map_err(internal)?;
        conn.execute(
            "UPDATE tickets SET sub_issue_sort = ?2,
             updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'), version = version + 1
             WHERE id = ?1",
            params![ticket_id, next],
        )
        .map_err(internal)?;
        Ok(())
    }

    pub fn retire_label(
        &self,
        project_id: i64,
        name: &str,
    ) -> Result<kanban_app::IssueLabelRow, ApiError> {
        let conn = self.lock();
        let already_retired: Option<i64> = conn
            .query_row(
                "SELECT 1 FROM issue_labels
                 WHERE project_id = ?1 AND name = ?2 AND retired_at IS NOT NULL",
                params![project_id, name],
                |row| row.get(0),
            )
            .optional()
            .map_err(internal)?;
        if already_retired.is_some() {
            return Err(ApiError::invalid_request_field(
                "name",
                &format!("label `{name}` is already retired"),
            ));
        }
        let changed = conn
            .execute(
                "UPDATE issue_labels
                 SET retired_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
                 WHERE project_id = ?1 AND name = ?2 AND retired_at IS NULL",
                params![project_id, name],
            )
            .map_err(internal)?;
        if changed == 0 {
            return Err(ApiError::not_found(&format!("label `{name}`")));
        }
        let row: Option<kanban_app::IssueLabelRow> = conn
            .query_row(
                "SELECT l.name, l.color, g.name, COALESCE(g.single_select, 0),
                        l.retired_at IS NOT NULL
                 FROM issue_labels AS l
                 LEFT JOIN issue_label_groups AS g ON g.id = l.group_id
                 WHERE l.project_id = ?1 AND l.name = ?2",
                params![project_id, name],
                |row| {
                    Ok(kanban_app::IssueLabelRow {
                        name: row.get(0)?,
                        color: row.get(1)?,
                        group: row.get(2)?,
                        group_single_select: row.get::<_, i64>(3)? != 0,
                        retired: row.get(4)?,
                    })
                },
            )
            .optional()
            .map_err(internal)?;
        row.ok_or_else(|| ApiError::internal("the retired label row is missing"))
    }

    pub fn set_assignee(&self, ticket_id: i64, assignee: Option<&str>) -> Result<(), ApiError> {
        let conn = self.lock();
        let stored = assignee.unwrap_or("");
        let current: Option<String> = conn
            .query_row(
                "SELECT assignee FROM tickets WHERE id = ?1",
                [ticket_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(internal)?;
        if current.as_deref() == Some(stored) {
            return Ok(());
        }
        conn.execute(
            "UPDATE tickets SET assignee = ?2,
             updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'), version = version + 1
             WHERE id = ?1",
            params![ticket_id, stored],
        )
        .map_err(internal)?;
        Ok(())
    }

    pub fn set_creator(&self, ticket_id: i64, creator: &str) -> Result<(), ApiError> {
        let conn = self.lock();
        conn.execute(
            "UPDATE tickets SET creator = ?2 WHERE id = ?1",
            params![ticket_id, creator],
        )
        .map_err(internal)?;
        Ok(())
    }

    pub fn sub_issue_progress(&self, ticket_id: i64) -> Result<(u32, u32), ApiError> {
        let conn = self.lock();
        let total: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM tickets WHERE parent_ticket_id = ?1",
                [ticket_id],
                |row| row.get(0),
            )
            .map_err(internal)?;
        let done: i64 = conn
            .query_row(
                "SELECT COUNT(*)
                 FROM tickets AS t
                 JOIN issue_workflow_states AS s ON s.id = t.workflow_state_id
                 WHERE t.parent_ticket_id = ?1
                   AND s.type IN ('completed', 'canceled', 'duplicate')",
                [ticket_id],
                |row| row.get(0),
            )
            .map_err(internal)?;
        Ok((done as u32, total as u32))
    }

    pub fn add_blocked_by(&self, ticket_id: i64, blocker_identifier: &str) -> Result<(), ApiError> {
        let conn = self.lock();
        let Some(blocker_id) =
            resolve_relation_target(&conn, blocker_identifier).map_err(internal_storage)?
        else {
            return Err(ApiError::not_found(blocker_identifier));
        };
        drop(conn);
        if blocker_id == ticket_id {
            return Err(ApiError::invalid_request_field(
                "add_blocked_by",
                "an issue cannot block itself",
            ));
        }
        let key = self.load_issue_key(ticket_id)?;
        self.add_relation(blocker_id, &key.display(), "blocks")
    }

    pub fn remove_blocked_by(
        &self,
        ticket_id: i64,
        blocker_identifier: &str,
    ) -> Result<(), ApiError> {
        let conn = self.lock();
        let Some(blocker_id) =
            resolve_relation_target(&conn, blocker_identifier).map_err(internal_storage)?
        else {
            return Err(ApiError::not_found(blocker_identifier));
        };
        drop(conn);
        let key = self.load_issue_key(ticket_id)?;
        self.remove_relation(blocker_id, &key.display(), "blocks")
    }

    pub fn sync_description_mentions(
        &self,
        ticket_id: i64,
        description: &str,
    ) -> Result<(), ApiError> {
        let self_key = self.load_issue_key(ticket_id)?.display();
        for identifier in mentioned_issue_keys(description) {
            if identifier.eq_ignore_ascii_case(&self_key) {
                continue;
            }
            let _ = self.add_relation(ticket_id, &identifier, "related");
        }
        Ok(())
    }

    pub fn set_auto_close_sub_issues(
        &self,
        project_id: i64,
        enabled: bool,
    ) -> Result<(), ApiError> {
        let conn = self.lock();
        let changed = conn
            .execute(
                "UPDATE projects SET auto_close_sub_issues = ?2 WHERE id = ?1",
                params![project_id, i64::from(enabled)],
            )
            .map_err(internal)?;
        if changed == 0 {
            return Err(ApiError::not_found("project"));
        }
        Ok(())
    }

    pub fn auto_close_sub_issues_enabled(&self, project_id: i64) -> Result<bool, ApiError> {
        let conn = self.lock();
        let enabled: i64 = conn
            .query_row(
                "SELECT auto_close_sub_issues FROM projects WHERE id = ?1",
                [project_id],
                |row| row.get(0),
            )
            .map_err(|_| ApiError::not_found("project"))?;
        Ok(enabled != 0)
    }
    pub fn set_close_sub_issues_with_parent(
        &self,
        project_id: i64,
        enabled: bool,
    ) -> Result<(), ApiError> {
        let conn = self.lock();
        let changed = conn
            .execute(
                "UPDATE projects SET close_sub_issues_with_parent = ?2 WHERE id = ?1",
                params![project_id, i64::from(enabled)],
            )
            .map_err(internal)?;
        if changed == 0 {
            return Err(ApiError::not_found("project"));
        }
        Ok(())
    }

    pub fn close_sub_issues_with_parent_enabled(&self, project_id: i64) -> Result<bool, ApiError> {
        let conn = self.lock();
        let enabled: i64 = conn
            .query_row(
                "SELECT close_sub_issues_with_parent FROM projects WHERE id = ?1",
                [project_id],
                |row| row.get(0),
            )
            .map_err(|_| ApiError::not_found("project"))?;
        Ok(enabled != 0)
    }

    /// The project that owns one issue, or `None` when no such issue
    /// exists. Comment streams live under the owning project.
    pub fn issue_project(&self, ticket_id: i64) -> Result<Option<u64>, ApiError> {
        let conn = self.lock();
        let project: Option<i64> = conn
            .query_row(
                "SELECT project_id FROM tickets WHERE id = ?1",
                [ticket_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(internal)?;
        Ok(project.map(|value| value as u64))
    }

    /// Whether one non-ticket comment target names a stored entity of
    /// its kind. Streams outside the issue model keep their raw id,
    /// so existence is the only membership check available.
    pub fn comment_target_exists(&self, kind: &str, id: &str) -> Result<bool, ApiError> {
        let conn = self.lock();
        let numeric_table = match kind {
            "initiative" => Some("initiatives"),
            "project" => Some("projects"),
            "plan" => Some("plans"),
            "spec" => Some("specs"),
            "run" => Some("runs"),
            "review" => Some("review_executions"),
            "evidence" => Some("evidence_items"),
            "comment" => Some("comments"),
            "workspace" => Some("workspaces"),
            "lane" => Some("lanes"),
            "profile" => Some("execution_profiles"),
            _ => None,
        };
        if let Some(table) = numeric_table {
            let Ok(numeric) = id.parse::<i64>() else {
                return Ok(false);
            };
            let found: Option<i64> = conn
                .query_row(
                    &format!("SELECT 1 FROM {table} WHERE id = ?1"),
                    params![numeric],
                    |row| row.get(0),
                )
                .optional()
                .map_err(internal)?;
            return Ok(found.is_some());
        }
        match kind {
            "attention_item" => {
                let found: Option<i64> = conn
                    .query_row(
                        "SELECT 1 FROM attention_items WHERE id = ?1",
                        params![id],
                        |row| row.get(0),
                    )
                    .optional()
                    .map_err(internal)?;
                Ok(found.is_some())
            }
            "finding" => {
                let Some(identity) = kanban_domain::finding::FindingIdentity::parse(id).ok() else {
                    return Ok(false);
                };
                let record: Option<String> = conn
                    .query_row(
                        "SELECT v.record FROM review_slot_verdicts AS v
                         JOIN review_slots AS s ON s.id = v.slot_id
                         WHERE s.id = ?1 AND s.review_id = ?2",
                        params![identity.slot() as i64, identity.review() as i64],
                        |row| row.get(0),
                    )
                    .optional()
                    .map_err(internal)?;
                let Some(record) = record else {
                    return Ok(false);
                };
                let verdict: serde_json::Value = serde_json::from_str(&record)
                    .map_err(|error| ApiError::internal(&error.to_string()))?;
                Ok(verdict["findings"]
                    .as_array()
                    .is_some_and(|findings| findings.len() > identity.index() as usize))
            }
            _ => Err(ApiError::internal(&format!(
                "comment target kind `{kind}` has no existence check"
            ))),
        }
    }

    /// The project that owns one non-ticket comment target of its
    /// kind, or `None` when the kind carries no owning project. The
    /// text-project columns hold the numeric id as text, the way the
    /// comment rows themselves do.
    pub fn comment_target_project(&self, kind: &str, id: &str) -> Result<Option<u64>, ApiError> {
        let conn = self.lock();
        let integer_project = |sql: &str, parameter: i64| -> Result<Option<u64>, ApiError> {
            let project: Option<i64> = conn
                .query_row(sql, params![parameter], |row| row.get(0))
                .optional()
                .map_err(internal)?;
            Ok(project.map(|value| value as u64))
        };
        let text_project = |sql: &str, parameter: &str| -> Result<Option<u64>, ApiError> {
            let project: Option<String> = conn
                .query_row(sql, params![parameter], |row| row.get(0))
                .optional()
                .map_err(internal)?;
            Ok(project.and_then(|value| value.parse().ok()))
        };
        match kind {
            "project" => Ok(Some(id.parse::<i64>().map_err(|_| {
                ApiError::internal("a resolved project target carries its numeric id")
            })? as u64)),
            "plan" => integer_project(
                "SELECT project_id FROM plans WHERE id = ?1",
                id.parse()
                    .map_err(|_| ApiError::internal("a resolved plan target is numeric"))?,
            ),
            "spec" => integer_project(
                "SELECT project_id FROM specs WHERE id = ?1",
                id.parse()
                    .map_err(|_| ApiError::internal("a resolved spec target is numeric"))?,
            ),
            "run" => integer_project(
                "SELECT project_id FROM runs WHERE id = ?1",
                id.parse()
                    .map_err(|_| ApiError::internal("a resolved run target is numeric"))?,
            ),
            "review" => integer_project(
                "SELECT project_id FROM review_executions WHERE id = ?1",
                id.parse()
                    .map_err(|_| ApiError::internal("a resolved review target is numeric"))?,
            ),
            "lane" => integer_project(
                "SELECT project_id FROM lanes WHERE id = ?1",
                id.parse()
                    .map_err(|_| ApiError::internal("a resolved lane target is numeric"))?,
            ),
            "attention_item" => {
                // The attention item's identifier is free text, the
                // way its existence check reads it; only its project
                // column is numeric.
                let project: Option<i64> = conn
                    .query_row(
                        "SELECT project_id FROM attention_items WHERE id = ?1",
                        params![id],
                        |row| row.get(0),
                    )
                    .optional()
                    .map_err(internal)?;
                Ok(project.map(|value| value as u64))
            }
            "workspace" => integer_project(
                "SELECT project_id FROM workspaces WHERE id = ?1",
                id.parse()
                    .map_err(|_| ApiError::internal("a resolved workspace target is numeric"))?,
            ),
            "evidence" => text_project("SELECT project_id FROM evidence_items WHERE id = ?1", id),
            "comment" => text_project("SELECT project_id FROM comments WHERE id = ?1", id),
            "finding" => {
                let identity = kanban_domain::finding::FindingIdentity::parse(id)
                    .map_err(|_| ApiError::internal("a resolved finding target is valid"))?;
                integer_project(
                    "SELECT r.project_id FROM review_slots AS s
                     JOIN review_executions AS r ON r.id = s.review_id
                     WHERE s.id = ?1",
                    identity.slot() as i64,
                )
            }
            _ => Ok(None),
        }
    }

    /// Refuse a label-add batch that cannot land: an unknown or
    /// retired label, or two distinct labels from one single-select
    /// group, in the order the caller listed them.
    pub fn validate_label_adds(&self, project_id: i64, names: &[String]) -> Result<(), ApiError> {
        let conn = self.lock();
        let mut held: HashMap<String, String> = HashMap::new();
        for name in names {
            let row: Option<(Option<String>, i64, Option<String>)> = conn
                .query_row(
                    "SELECT g.name, COALESCE(g.single_select, 0), l.retired_at
                     FROM issue_labels AS l
                     LEFT JOIN issue_label_groups AS g ON g.id = l.group_id
                     WHERE l.project_id = ?1 AND l.name = ?2",
                    params![project_id, name],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .optional()
                .map_err(internal)?;
            let Some((group, single_select, retired_at)) = row else {
                return Err(ApiError::invalid_request_field(
                    "add_labels",
                    &format!("unknown label `{name}`"),
                ));
            };
            if retired_at.is_some() {
                return Err(ApiError::invalid_request_field(
                    "add_labels",
                    &format!("label `{name}` is retired"),
                ));
            }
            if single_select == 1
                && let Some(group_name) = group.as_deref()
                && let Some(first_label) = held.get(group_name)
                && first_label != name
            {
                return Err(ApiError::invalid_request_field(
                    "add_labels",
                    &format!(
                        "labels `{first_label}` and `{name}` are both from the \
                         single-select group `{group_name}`; add one of them"
                    ),
                ));
            }
            if single_select == 1
                && let Some(group_name) = group
            {
                held.entry(group_name).or_insert_with(|| name.clone());
            }
        }
        Ok(())
    }

    pub fn create_milestone(
        &self,
        project_id: i64,
        name: &str,
        target_date: Option<&str>,
    ) -> Result<kanban_app::IssueMilestoneRow, ApiError> {
        let target_date = target_date
            .map(|raw| validate_calendar_date(raw, "target_date"))
            .transpose()?;
        let conn = self.lock();
        conn.execute(
            "INSERT INTO issue_milestones (project_id, name, sort_order, target_date)
             VALUES (?1, ?2, (SELECT COALESCE(MAX(sort_order), -1) + 1 FROM issue_milestones WHERE project_id = ?1), ?3)",
            params![project_id, name, target_date],
        )
        .map_err(|error| taken_name_refusal(&conn, NamedKind::Milestone, project_id, name, error))?;
        let id = conn.last_insert_rowid() as u64;
        Ok(kanban_app::IssueMilestoneRow {
            id,
            name: name.to_owned(),
            target_date: target_date.as_deref().map(str::to_owned),
            issues_total: 0,
            issues_done: 0,
        })
    }

    pub fn create_label(
        &self,
        project_id: i64,
        name: &str,
        color: &str,
        group: Option<&str>,
        group_single_select: bool,
    ) -> Result<kanban_app::IssueLabelRow, ApiError> {
        let conn = self.lock();
        let group_id = if let Some(group_name) = group {
            conn.execute(
                "INSERT OR IGNORE INTO issue_label_groups (project_id, name, single_select)
                 VALUES (?1, ?2, ?3)",
                params![project_id, group_name, i64::from(group_single_select)],
            )
            .map_err(internal)?;
            Some(
                conn.query_row(
                    "SELECT id FROM issue_label_groups WHERE project_id = ?1 AND name = ?2",
                    params![project_id, group_name],
                    |row| row.get::<_, i64>(0),
                )
                .map_err(internal)?,
            )
        } else {
            None
        };
        conn.execute(
            "INSERT INTO issue_labels (project_id, name, color, group_id)
             VALUES (?1, ?2, ?3, ?4)",
            params![project_id, name, color, group_id],
        )
        .map_err(|error| taken_name_refusal(&conn, NamedKind::Label, project_id, name, error))?;
        let stored_single_select = if let Some(group_id) = group_id {
            conn.query_row(
                "SELECT COALESCE(single_select, 0) FROM issue_label_groups WHERE id = ?1",
                [group_id],
                |row| row.get::<_, i64>(0),
            )
            .map_err(internal)?
                != 0
        } else {
            group_single_select
        };
        Ok(kanban_app::IssueLabelRow {
            name: name.to_owned(),
            color: color.to_owned(),
            group: group.map(str::to_owned),
            group_single_select: stored_single_select,
            retired: false,
        })
    }

    pub fn list_relations(&self, ticket_id: i64) -> Result<Vec<IssueRelationRecord>, ApiError> {
        let conn = self.lock();
        let mut statement = conn
            .prepare(
                "SELECT 'blocks' AS kind, p.code, t.number
                 FROM ticket_dependencies AS d
                 JOIN tickets AS t ON t.id = d.to_ticket
                 JOIN projects AS p ON p.id = t.project_id
                 WHERE d.from_ticket = ?1
                 UNION ALL
                 SELECT 'blocked-by', p.code, t.number
                 FROM ticket_dependencies AS d
                 JOIN tickets AS t ON t.id = d.from_ticket
                 JOIN projects AS p ON p.id = t.project_id
                 WHERE d.to_ticket = ?1
                 UNION ALL
                 SELECT 'related' AS kind, p.code, t.number
                 FROM issue_relations AS r
                 JOIN tickets AS t ON t.id = r.to_ticket_id
                 JOIN projects AS p ON p.id = t.project_id
                 WHERE r.from_ticket_id = ?1 AND r.kind = 'related'
                 UNION ALL
                 SELECT 'related', p.code, t.number
                 FROM issue_relations AS r
                 JOIN tickets AS t ON t.id = r.from_ticket_id
                 JOIN projects AS p ON p.id = t.project_id
                 WHERE r.to_ticket_id = ?1 AND r.kind = 'related'
                 ORDER BY kind, code, number",
            )
            .map_err(internal)?;
        let rows = statement
            .query_map([ticket_id], |row| {
                let code: String = row.get(1)?;
                let number: i64 = row.get(2)?;
                Ok(IssueRelationRecord {
                    kind: row.get(0)?,
                    identifier: format!("{code}-{number}"),
                })
            })
            .map_err(internal)?;
        let mut rows = rows.collect::<Result<Vec<_>, _>>().map_err(internal)?;
        rows.dedup();
        Ok(rows)
    }
}

fn mentioned_issue_keys(description: &str) -> Vec<String> {
    let mut mentioned = Vec::new();
    for token in description.split_whitespace() {
        push_issue_key(
            &mut mentioned,
            token.trim_matches(|c: char| !c.is_alphanumeric() && c != '-'),
        );
        if let Some(rest) = token.split("kanban://issue/").nth(1) {
            let key: String = rest
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '-')
                .collect();
            push_issue_key(&mut mentioned, &key);
        }
    }
    let mut search = 0;
    while let Some(rel) = description[search..].find("](") {
        let close = search + rel;
        let Some(open_rel) = description[search..close]
            .char_indices()
            .rev()
            .find(|(_, ch)| *ch == '[')
            .map(|(index, _)| index)
        else {
            search = close + 2;
            continue;
        };
        let start = search + open_rel;
        push_issue_key(&mut mentioned, description[start + 1..close].trim());
        let target_start = close + 2;
        let Some(end_rel) = description[target_start..].find(')') else {
            search = target_start;
            continue;
        };
        let target_end = target_start + end_rel;
        let target = &description[target_start..target_end];
        push_issue_key(&mut mentioned, target.trim());
        if let Some(rest) = target.split("kanban://issue/").nth(1) {
            let key: String = rest
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '-')
                .collect();
            push_issue_key(&mut mentioned, &key);
        }
        search = target_end + 1;
    }
    push_issue_tag_mentions(&mut mentioned, description);
    mentioned.sort();
    mentioned.dedup();
    mentioned
}

fn push_issue_key(mentioned: &mut Vec<String>, raw: &str) {
    if kanban_domain::IssueKey::parse(raw).is_some() {
        mentioned.push(raw.to_owned());
    }
}

fn push_issue_tag_mentions(mentioned: &mut Vec<String>, description: &str) {
    let lower = description.to_ascii_lowercase();
    let mut search = 0;
    while let Some(rel) = lower[search..].find("<issue") {
        let start = search + rel;
        let Some(end_rel) = lower[start..].find("</issue>") else {
            break;
        };
        let end = start + end_rel;
        let tag = &description[start..end];
        let body_at = tag.find('>').unwrap_or(tag.len());
        let opening = &tag[..body_at];
        let body = tag.get(body_at + 1..).unwrap_or("");
        push_issue_key(mentioned, body.trim());
        for attribute in ["id", "identifier"] {
            if let Some(value) = attribute_value(opening, attribute) {
                push_issue_key(mentioned, value.trim());
            }
        }
        search = end + "</issue>".len();
    }
}

fn attribute_value<'a>(opening: &'a str, name: &str) -> Option<&'a str> {
    let lower = opening.to_ascii_lowercase();
    let needle = format!("{name}=\"");
    let start = lower.find(&needle)? + needle.len();
    let rest = opening.get(start..)?;
    rest.split('"').next()
}

fn map_workflow_state(record: WorkflowStateRecord) -> kanban_app::IssueWorkflowState {
    kanban_app::IssueWorkflowState {
        id: record.id,
        project_id: record.project_id,
        name: record.name,
        state_type: record.state_type,
        legacy_state: record.legacy_state,
    }
}

impl kanban_app::IssueStore for SqliteIssueStore {
    fn set_description(&self, ticket_id: i64, description: &str) -> Result<(), ApiError> {
        SqliteIssueStore::set_description(self, ticket_id, description)
    }

    fn patch_description(&self, ticket_id: i64, patch: &str) -> Result<String, ApiError> {
        SqliteIssueStore::patch_description(self, ticket_id, patch)
    }

    fn set_linear_priority(&self, ticket_id: i64, priority: u8) -> Result<(), ApiError> {
        SqliteIssueStore::set_linear_priority(self, ticket_id, priority)
    }

    fn set_initial_linear_priority(&self, ticket_id: i64, priority: u8) -> Result<(), ApiError> {
        SqliteIssueStore::set_initial_linear_priority(self, ticket_id, priority)
    }

    fn set_due_date(&self, ticket_id: i64, due_date: Option<&str>) -> Result<(), ApiError> {
        SqliteIssueStore::set_due_date(self, ticket_id, due_date)
    }

    fn set_milestone(&self, ticket_id: i64, milestone_id: Option<i64>) -> Result<(), ApiError> {
        SqliteIssueStore::set_milestone(self, ticket_id, milestone_id)
    }

    fn apply_free_state(
        &self,
        ticket_id: i64,
        project_id: i64,
        to_state: &kanban_app::IssueWorkflowState,
        envelope: kanban_app::TimelineEnvelope,
    ) -> Result<(), ApiError> {
        let record = WorkflowStateRecord {
            id: to_state.id,
            project_id: to_state.project_id,
            name: to_state.name.clone(),
            state_type: to_state.state_type,
            legacy_state: to_state.legacy_state.clone(),
            position: 0,
        };
        SqliteIssueStore::apply_free_state(self, ticket_id, project_id, &record, envelope)
    }

    fn record_initial_state(&self, ticket_id: i64, state_name: &str) -> Result<(), ApiError> {
        SqliteIssueStore::record_initial_state(self, ticket_id, state_name)
    }

    fn list_labels(&self, ticket_id: i64) -> Result<Vec<String>, ApiError> {
        SqliteIssueStore::list_labels(self, ticket_id)
    }

    fn add_label(&self, project_id: i64, ticket_id: i64, name: &str) -> Result<(), ApiError> {
        SqliteIssueStore::add_label(self, project_id, ticket_id, name)
    }

    fn remove_label(&self, project_id: i64, ticket_id: i64, name: &str) -> Result<(), ApiError> {
        SqliteIssueStore::remove_label(self, project_id, ticket_id, name)
    }

    fn load_issue_key(&self, ticket_id: i64) -> Result<IssueKey, ApiError> {
        SqliteIssueStore::load_issue_key(self, ticket_id)
    }

    fn workflow_state_for_ticket(
        &self,
        ticket_id: i64,
    ) -> Result<Option<kanban_app::IssueWorkflowState>, ApiError> {
        SqliteIssueStore::workflow_state_for_ticket(self, ticket_id)
            .map(|state| state.map(map_workflow_state))
    }

    fn resolve_identifier(&self, identifier: &str) -> Result<Option<(IssueKey, i64)>, ApiError> {
        SqliteIssueStore::resolve_identifier(self, identifier)
    }

    fn list_workflow_states(
        &self,
        project_id: i64,
    ) -> Result<Vec<kanban_app::IssueWorkflowState>, ApiError> {
        SqliteIssueStore::list_workflow_states_for_project(self, project_id)
            .map(|states| states.into_iter().map(map_workflow_state).collect())
    }

    fn list_project_labels(
        &self,
        project_id: i64,
    ) -> Result<Vec<kanban_app::IssueLabelRow>, ApiError> {
        SqliteIssueStore::list_project_labels(self, project_id)
    }

    fn list_label_groups(&self, project_id: i64) -> Result<Vec<String>, ApiError> {
        SqliteIssueStore::list_label_groups(self, project_id)
    }

    fn list_project_milestones(
        &self,
        project_id: i64,
    ) -> Result<Vec<kanban_app::IssueMilestoneRow>, ApiError> {
        SqliteIssueStore::list_project_milestones(self, project_id)
    }

    fn resolve_state_by_name(
        &self,
        project_id: i64,
        name: &str,
    ) -> Result<kanban_app::IssueWorkflowState, ApiError> {
        SqliteIssueStore::resolve_state_by_name(self, project_id, name).map(map_workflow_state)
    }

    fn issue_fields(&self, ticket_id: i64) -> Result<kanban_app::IssueTicketFields, ApiError> {
        SqliteIssueStore::issue_fields(self, ticket_id)
    }

    fn list_relations(&self, ticket_id: i64) -> Result<Vec<IssueRelationRecord>, ApiError> {
        SqliteIssueStore::list_relations(self, ticket_id)
    }

    fn list_state_history(
        &self,
        ticket_id: i64,
    ) -> Result<Vec<kanban_dto::IssueStateHistoryRecord>, ApiError> {
        SqliteIssueStore::list_state_history(self, ticket_id)
    }

    fn set_title(&self, ticket_id: i64, title: &str) -> Result<(), ApiError> {
        SqliteIssueStore::set_title(self, ticket_id, title)
    }

    fn bump_version(&self, ticket_id: i64) -> Result<u64, ApiError> {
        SqliteIssueStore::bump_version(self, ticket_id)
    }

    fn set_parent(&self, ticket_id: i64, parent_id: Option<i64>) -> Result<(), ApiError> {
        SqliteIssueStore::set_parent(self, ticket_id, parent_id)
    }

    fn list_children(&self, ticket_id: i64) -> Result<Vec<kanban_dto::IssueChildRecord>, ApiError> {
        SqliteIssueStore::list_children(self, ticket_id)
    }

    fn add_relation(
        &self,
        from_ticket_id: i64,
        to_identifier: &str,
        kind: &str,
    ) -> Result<(), ApiError> {
        SqliteIssueStore::add_relation(self, from_ticket_id, to_identifier, kind)
    }

    fn remove_relation(
        &self,
        from_ticket_id: i64,
        to_identifier: &str,
        kind: &str,
    ) -> Result<(), ApiError> {
        SqliteIssueStore::remove_relation(self, from_ticket_id, to_identifier, kind)
    }

    fn set_duplicate_of(
        &self,
        ticket_id: i64,
        target_identifier: Option<&str>,
    ) -> Result<(), ApiError> {
        SqliteIssueStore::set_duplicate_of(self, ticket_id, target_identifier)
    }

    fn set_sub_issue_sort(&self, ticket_id: i64, sort: f64) -> Result<(), ApiError> {
        SqliteIssueStore::set_sub_issue_sort(self, ticket_id, sort)
    }

    fn list_duplicate_identifiers(&self, ticket_id: i64) -> Result<Vec<String>, ApiError> {
        SqliteIssueStore::list_duplicate_identifiers(self, ticket_id)
    }

    fn sub_issue_progress(&self, ticket_id: i64) -> Result<(u32, u32), ApiError> {
        SqliteIssueStore::sub_issue_progress(self, ticket_id)
    }

    fn add_blocked_by(&self, ticket_id: i64, blocker_identifier: &str) -> Result<(), ApiError> {
        SqliteIssueStore::add_blocked_by(self, ticket_id, blocker_identifier)
    }

    fn remove_blocked_by(&self, ticket_id: i64, blocker_identifier: &str) -> Result<(), ApiError> {
        SqliteIssueStore::remove_blocked_by(self, ticket_id, blocker_identifier)
    }

    fn sync_description_mentions(&self, ticket_id: i64, description: &str) -> Result<(), ApiError> {
        SqliteIssueStore::sync_description_mentions(self, ticket_id, description)
    }

    fn set_auto_close_sub_issues(&self, project_id: i64, enabled: bool) -> Result<(), ApiError> {
        SqliteIssueStore::set_auto_close_sub_issues(self, project_id, enabled)
    }

    fn auto_close_sub_issues_enabled(&self, project_id: i64) -> Result<bool, ApiError> {
        SqliteIssueStore::auto_close_sub_issues_enabled(self, project_id)
    }
    fn set_close_sub_issues_with_parent(
        &self,
        project_id: i64,
        enabled: bool,
    ) -> Result<(), ApiError> {
        SqliteIssueStore::set_close_sub_issues_with_parent(self, project_id, enabled)
    }

    fn close_sub_issues_with_parent_enabled(&self, project_id: i64) -> Result<bool, ApiError> {
        SqliteIssueStore::close_sub_issues_with_parent_enabled(self, project_id)
    }

    fn issue_project(&self, ticket_id: i64) -> Result<Option<u64>, ApiError> {
        SqliteIssueStore::issue_project(self, ticket_id)
    }

    fn comment_target_exists(&self, kind: &str, id: &str) -> Result<bool, ApiError> {
        SqliteIssueStore::comment_target_exists(self, kind, id)
    }

    fn comment_target_project(&self, kind: &str, id: &str) -> Result<Option<u64>, ApiError> {
        SqliteIssueStore::comment_target_project(self, kind, id)
    }

    fn validate_label_adds(&self, project_id: i64, names: &[String]) -> Result<(), ApiError> {
        SqliteIssueStore::validate_label_adds(self, project_id, names)
    }

    fn touch_updated_at(&self, ticket_id: i64) -> Result<(), ApiError> {
        SqliteIssueStore::touch_updated_at(self, ticket_id)
    }

    fn page_issue_ids(
        &self,
        query: &kanban_app::IssuePageQuery<'_>,
    ) -> Result<kanban_app::IssuePage, ApiError> {
        page_issue_ids(&self.lock(), query)
    }

    fn create_milestone(
        &self,
        project_id: i64,
        name: &str,
        target_date: Option<&str>,
    ) -> Result<kanban_app::IssueMilestoneRow, ApiError> {
        SqliteIssueStore::create_milestone(self, project_id, name, target_date)
    }

    fn create_label(
        &self,
        project_id: i64,
        name: &str,
        color: &str,
        group: Option<&str>,
        group_single_select: bool,
    ) -> Result<kanban_app::IssueLabelRow, ApiError> {
        SqliteIssueStore::create_label(self, project_id, name, color, group, group_single_select)
    }

    fn append_sub_issue_sort(&self, ticket_id: i64, parent_id: i64) -> Result<(), ApiError> {
        SqliteIssueStore::append_sub_issue_sort(self, ticket_id, parent_id)
    }

    fn retire_label(
        &self,
        project_id: i64,
        name: &str,
    ) -> Result<kanban_app::IssueLabelRow, ApiError> {
        SqliteIssueStore::retire_label(self, project_id, name)
    }

    fn set_assignee(&self, ticket_id: i64, assignee: Option<&str>) -> Result<(), ApiError> {
        SqliteIssueStore::set_assignee(self, ticket_id, assignee)
    }

    fn set_creator(&self, ticket_id: i64, creator: &str) -> Result<(), ApiError> {
        SqliteIssueStore::set_creator(self, ticket_id, creator)
    }

    fn begin_secondary_effects(&self) {
        open_secondary_effects();
    }

    fn take_secondary_effects(&self) -> Vec<kanban_app::SecondaryEffect> {
        drain_secondary_effects()
    }
}

/// Land one free state move over `conn`: the workflow row, one open
/// history row, the lifecycle stamps, the timeline event, and the
/// guarded version bump, nesting inside any write span already open.
pub(crate) fn apply_free_state_on(
    conn: &rusqlite::Connection,
    ticket_id: i64,
    project_id: i64,
    to_state: &WorkflowStateRecord,
    envelope: TimelineEnvelope,
) -> Result<(), ApiError> {
    let span = WriteSpan::begin(conn).map_err(internal)?;
    let now: String = span
        .query_row("SELECT strftime('%Y-%m-%dT%H:%M:%fZ', 'now')", [], |row| {
            row.get(0)
        })
        .map_err(internal)?;
    let mut stamps = IssueStateTimestamps {
        started_at: span
            .query_row(
                "SELECT started_at FROM tickets WHERE id = ?1",
                [ticket_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(internal)?
            .flatten(),
        completed_at: span
            .query_row(
                "SELECT completed_at FROM tickets WHERE id = ?1",
                [ticket_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(internal)?
            .flatten(),
        canceled_at: span
            .query_row(
                "SELECT canceled_at FROM tickets WHERE id = ?1",
                [ticket_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(internal)?
            .flatten(),
    };
    stamps.stamp_entry(to_state.state_type, &now);
    stamps.clear_terminal_on_reopen(to_state.state_type);
    let (created_at, prior_workflow): (String, Option<i64>) = span
        .query_row(
            "SELECT created_at, workflow_state_id FROM tickets WHERE id = ?1",
            [ticket_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(internal)?;
    if prior_workflow == Some(to_state.id) {
        span.commit().map_err(internal)?;
        return Ok(());
    }
    let history_rows: i64 = span
        .query_row(
            "SELECT COUNT(*) FROM issue_state_history WHERE ticket_id = ?1",
            [ticket_id],
            |row| row.get(0),
        )
        .map_err(internal)?;
    if history_rows == 0
        && let Some(from_id) = prior_workflow
    {
        span.execute(
            "INSERT INTO issue_state_history (ticket_id, workflow_state_id, started_at, ended_at)
                 VALUES (?1, ?2, ?3, ?4)",
            params![ticket_id, from_id, &created_at, &now],
        )
        .map_err(internal)?;
    }
    span.execute(
        "UPDATE issue_state_history SET ended_at = ?2
             WHERE ticket_id = ?1 AND ended_at IS NULL",
        params![ticket_id, &now],
    )
    .map_err(internal)?;
    span.execute(
        "INSERT INTO issue_state_history (ticket_id, workflow_state_id, started_at, ended_at)
             VALUES (?1, ?2, ?3, NULL)",
        params![ticket_id, to_state.id, &now],
    )
    .map_err(internal)?;
    let legacy = TicketState::parse(&to_state.legacy_state)
        .ok_or_else(|| ApiError::internal("stored workflow state is invalid"))?;
    note_secondary_effect(&span, ticket_id);
    span.execute(
        "UPDATE tickets
             SET state = ?2,
                 workflow_state_id = ?3,
                 started_at = ?4,
                 completed_at = ?5,
                 canceled_at = ?6,
                 updated_at = ?7,
                 version = version + 1
             WHERE id = ?1",
        params![
            ticket_id,
            legacy.wire_name(),
            to_state.id,
            stamps.started_at,
            stamps.completed_at,
            stamps.canceled_at,
            &now,
        ],
    )
    .map_err(internal)?;
    insert_event(&span, &envelope).map_err(|error| ApiError::internal(&error.to_string()))?;
    let parent_id: Option<i64> = span
        .query_row(
            "SELECT parent_ticket_id FROM tickets WHERE id = ?1",
            [ticket_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(internal)?
        .flatten();
    let _ = project_id;
    span.commit().map_err(internal)?;
    if to_state.state_type.is_closed() {
        close_open_sub_issues_with_parent(conn, ticket_id, to_state.state_type)?;
        if let Some(parent_id) = parent_id {
            auto_close_parent_if_children_done(conn, parent_id)?;
        }
    }
    Ok(())
}

/// Close the open sub-issues of a parent that has just entered
/// `parent_state`, when the parent's project closes sub-issues with
/// their parent (Linear's "auto-close sub-issues"). The domain picks the
/// state type; each sub-issue lands on the first state of that type in
/// its own project's workflow through the same free move as any other,
/// so it gets its history row, stamps, timeline event, and version
/// bump, and closes its own open sub-issues in turn. Sub-issues already
/// closed keep their state.
///
/// The rule only closes open issues and parent auto-close only closes
/// open parents, so the two never undo or re-enter each other.
pub(crate) fn close_open_sub_issues_with_parent(
    conn: &rusqlite::Connection,
    parent_id: i64,
    parent_state: WorkflowStateType,
) -> Result<(), ApiError> {
    let Some(closure) = parent_state.sub_issue_closure() else {
        return Ok(());
    };
    let enabled: i64 = conn
        .query_row(
            "SELECT p.close_sub_issues_with_parent
             FROM tickets AS t
             JOIN projects AS p ON p.id = t.project_id
             WHERE t.id = ?1",
            [parent_id],
            |row| row.get(0),
        )
        .map_err(not_found_ticket)?;
    if enabled == 0 {
        return Ok(());
    }
    let children: Vec<(i64, i64, Option<String>)> = {
        let mut statement = conn
            .prepare(
                "SELECT t.id, t.project_id, s.type
                 FROM tickets AS t
                 LEFT JOIN issue_workflow_states AS s ON s.id = t.workflow_state_id
                 WHERE t.parent_ticket_id = ?1
                 ORDER BY t.sub_issue_sort, t.number",
            )
            .map_err(internal)?;
        statement
            .query_map([parent_id], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })
            .map_err(internal)?
            .collect::<Result<_, _>>()
            .map_err(internal)?
    };
    for (child_id, child_project, child_type) in children {
        let open = child_type
            .as_deref()
            .and_then(WorkflowStateType::parse)
            .is_none_or(|state| !state.is_closed());
        if !open {
            continue;
        }
        // The same rule parent auto-close follows: a live run keeps the
        // issue open. Leaving it unmoved records the refusal the way any
        // refused move does — no version bump and no timeline row.
        let executing: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM runs WHERE ticket_id = ?1 AND status = 'executing'",
                [child_id],
                |row| row.get(0),
            )
            .map_err(internal)?;
        if executing > 0 {
            continue;
        }
        let target = first_workflow_state_of_type(conn, child_project, closure)
            .map_err(internal_storage)?
            .ok_or_else(|| {
                ApiError::internal(&format!(
                    "no {} workflow state to close a sub-issue into",
                    closure.wire_name()
                ))
            })?;
        let envelope = TimelineEnvelope::project(
            child_project as u64,
            TimelineEventKind::Transition,
            Some(TimelineEntityRef {
                kind: TimelineEntityKind::Ticket,
                id: child_id.to_string(),
            }),
            serde_json::json!({
                "action": "close_sub_issues_with_parent",
                "state": target.name,
                "parent_ticket_id": parent_id,
            }),
        );
        apply_free_state_on(conn, child_id, child_project, &target, envelope)?;
    }
    Ok(())
}
/// The parent auto-close rule over one connection, so every write
/// boundary (issue tools and legacy ticket moves alike) runs the same
/// rule inside its own transaction.
pub(crate) fn auto_close_parent_if_children_done(
    conn: &rusqlite::Connection,
    parent_id: i64,
) -> Result<(), ApiError> {
    let project_id: i64 = conn
        .query_row(
            "SELECT project_id FROM tickets WHERE id = ?1",
            [parent_id],
            |row| row.get(0),
        )
        .map_err(not_found_ticket)?;
    let enabled: i64 = conn
        .query_row(
            "SELECT auto_close_sub_issues FROM projects WHERE id = ?1",
            [project_id],
            |row| row.get(0),
        )
        .map_err(internal)?;
    if enabled == 0 {
        return Ok(());
    }
    let parent_type: Option<String> = conn
        .query_row(
            "SELECT s.type FROM tickets AS t
                 JOIN issue_workflow_states AS s ON s.id = t.workflow_state_id
                 WHERE t.id = ?1",
            [parent_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(internal)?
        .flatten();
    if parent_type
        .as_deref()
        .is_some_and(|kind| matches!(kind, "completed" | "canceled" | "duplicate"))
    {
        return Ok(());
    }
    let child_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM tickets WHERE parent_ticket_id = ?1",
            [parent_id],
            |row| row.get(0),
        )
        .map_err(internal)?;
    if child_count == 0 {
        return Ok(());
    }
    let open: i64 = conn
        .query_row(
            "SELECT COUNT(*)
                 FROM tickets AS t
                 LEFT JOIN issue_workflow_states AS s ON s.id = t.workflow_state_id
                 WHERE t.parent_ticket_id = ?1
                   AND (
                     t.workflow_state_id IS NULL
                     OR s.type NOT IN ('completed', 'canceled', 'duplicate')
                   )",
            [parent_id],
            |row| row.get(0),
        )
        .map_err(internal)?;
    if open > 0 {
        return Ok(());
    }
    let executing: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM runs WHERE ticket_id = ?1 AND status = 'executing'",
            [parent_id],
            |row| row.get(0),
        )
        .map_err(internal)?;
    if executing > 0 {
        return Ok(());
    }
    let done = workflow_state_by_name(conn, project_id, "Done")
        .map_err(internal_storage)?
        .ok_or_else(|| ApiError::internal("Done workflow state missing"))?;
    let envelope = TimelineEnvelope::project(
        project_id as u64,
        TimelineEventKind::Transition,
        Some(TimelineEntityRef {
            kind: TimelineEntityKind::Ticket,
            id: parent_id.to_string(),
        }),
        serde_json::json!({ "action": "auto_close_sub_issues", "state": "Done" }),
    );
    apply_free_state_on(conn, parent_id, project_id, &done, envelope)
}

/// The ticket a relation target names: an issue key in any case, or a
/// bare ticket id the way `parent_id` accepts one. `None` when no
/// ticket carries the reference.
fn resolve_relation_target(
    conn: &rusqlite::Connection,
    identifier: &str,
) -> Result<Option<i64>, StorageError> {
    if let Ok(id) = identifier.parse::<i64>() {
        let found: Option<i64> = conn
            .query_row("SELECT id FROM tickets WHERE id = ?1", [id], |row| {
                row.get(0)
            })
            .optional()
            .map_err(StorageError::from)?;
        return Ok(found);
    }
    Ok(super::linear_issue::resolve_issue_key(conn, identifier)?.map(|(_, id)| id))
}

fn page_issue_ids(
    conn: &rusqlite::Connection,
    query: &kanban_app::IssuePageQuery<'_>,
) -> Result<kanban_app::IssuePage, ApiError> {
    let mut sql = String::from(
        "SELECT t.id, t.updated_at FROM tickets AS t
         JOIN projects AS p ON p.id = t.project_id
         LEFT JOIN issue_workflow_states AS s ON s.id = t.workflow_state_id
         LEFT JOIN issue_workflow_states AS ls
                ON ls.project_id = t.project_id AND ls.legacy_state = t.state
         WHERE t.project_id IN (",
    );
    let mut params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
    for (index, project_id) in query.project_ids.iter().enumerate() {
        if index > 0 {
            sql.push(',');
        }
        sql.push_str(&format!("?{}", params.len() + 1));
        params.push(Box::new(*project_id as i64));
    }
    sql.push(')');
    if let Some(state) = query.state {
        let n = params.len() + 1;
        sql.push_str(&format!(
            " AND (fold_eq(COALESCE(s.name, ls.name), ?{n})
                OR fold_eq(COALESCE(s.type, ls.type), ?{n}))"
        ));
        params.push(Box::new(state.to_owned()));
    }
    if let Some(label) = query.label {
        let n = params.len() + 1;
        sql.push_str(&format!(
            " AND EXISTS (
                   SELECT 1 FROM issue_ticket_labels AS tl
                   JOIN issue_labels AS l ON l.id = tl.label_id
                   WHERE tl.ticket_id = t.id AND fold_eq(l.name, ?{n})
               )"
        ));
        params.push(Box::new(label.to_owned()));
    }
    if let Some(parent) = query.parent_ticket_id {
        sql.push_str(&format!(" AND t.parent_ticket_id = ?{}", params.len() + 1));
        params.push(Box::new(parent));
    }
    if let Some(milestone) = query.milestone_id {
        sql.push_str(&format!(" AND t.milestone_id = ?{}", params.len() + 1));
        params.push(Box::new(milestone));
    }
    if let Some(assignee) = query.assignee {
        sql.push_str(&format!(
            " AND COALESCE(t.assignee, '') = ?{}",
            params.len() + 1
        ));
        params.push(Box::new(assignee.to_owned()));
    }
    if let Some(floor) = query.updated_since_floor {
        let n = params.len() + 1;
        let m = params.len() + 2;
        sql.push_str(&format!(
            " AND (t.updated_at > ?{n} OR (t.updated_at = ?{n} AND ?{m}))"
        ));
        params.push(Box::new(floor.to_owned()));
        params.push(Box::new(i64::from(query.updated_since_holds_at_floor)));
    }
    if let Some(needle) = query.query {
        let n = params.len() + 1;
        sql.push_str(&format!(
            " AND issue_query_matches(
                   p.code || '-' || t.number,
                   COALESCE(CASE WHEN t.kind = 'implementation' THEN t.slice ELSE t.title END, ''),
                   COALESCE(t.description, ''),
                   ?{n}
               )"
        ));
        params.push(Box::new(needle.to_owned()));
    }
    if let Some(cursor) = query.cursor {
        let anchor_updated_at = if let Some(stamp) = query.cursor_updated_at {
            stamp.to_owned()
        } else {
            let anchor: Option<String> = conn
                .query_row(
                    "SELECT updated_at FROM tickets WHERE id = ?1",
                    [cursor as i64],
                    |row| row.get(0),
                )
                .optional()
                .map_err(internal)?;
            let Some(anchor_updated_at) = anchor else {
                return Err(ApiError::invalid_request_field(
                    "cursor",
                    "cursor does not match any listed issue",
                ));
            };
            anchor_updated_at
        };
        let n = params.len() + 1;
        let m = params.len() + 2;
        sql.push_str(&format!(
            " AND (t.updated_at < ?{n} OR (t.updated_at = ?{n} AND t.id < ?{m}))"
        ));
        params.push(Box::new(anchor_updated_at));
        params.push(Box::new(cursor as i64));
    }
    sql.push_str(" ORDER BY t.updated_at DESC, t.id DESC LIMIT ?");
    params.push(Box::new(query.limit as i64 + 1));
    let mut statement = conn.prepare(&sql).map_err(internal)?;
    let rows = statement
        .query_map(
            rusqlite::params_from_iter(params.iter().map(|value| value.as_ref())),
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
        )
        .map_err(internal)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(internal)?;
    let more = rows.len() > query.limit as usize;
    let kept: Vec<(i64, String)> = rows.into_iter().take(query.limit as usize).collect();
    let boundary = kept.last().cloned();
    Ok(kanban_app::IssuePage {
        ticket_ids: kept.iter().map(|(id, _)| *id as u64).collect(),
        more,
        boundary_updated_at: boundary.as_ref().map(|(_, stamp)| stamp.clone()),
        boundary_ticket_id: boundary.as_ref().map(|(id, _)| *id as u64),
    })
}

pub fn resolve_state_by_name(
    conn: &rusqlite::Connection,
    project_id: i64,
    name: &str,
) -> Result<WorkflowStateRecord, ApiError> {
    // A save matches state names the way the listings do — ignoring
    // case — and then a workflow state type, which resolves to the
    // project's first state of that type in board order, the way
    // Linear uses the team's default state for the type.
    let states =
        super::linear_issue::list_workflow_states(conn, project_id).map_err(internal_storage)?;
    let matches = |value: &str| focaccia::unicode_full_case_eq(value, name);
    states
        .iter()
        .find(|state| matches(&state.name))
        .or_else(|| {
            states
                .iter()
                .find(|state| matches(state.state_type.wire_name()))
        })
        .cloned()
        .ok_or_else(|| {
            let names = states
                .iter()
                .map(|state| state.name.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            ApiError::invalid_request_field(
                "state",
                &format!("unknown state `{name}`; valid states: {names}"),
            )
        })
}

/// Answer a label or milestone insert the database refused as a taken
/// name with the application's own judgement of that name, so the
/// constraint and the rule it backs refuse alike: an exact repeat
/// conflicts and a case variant (refused by migration 0066's triggers)
/// is an invalid name quoting the stored spelling.
fn taken_name_refusal(
    conn: &rusqlite::Connection,
    kind: NamedKind,
    project_id: i64,
    name: &str,
    error: rusqlite::Error,
) -> ApiError {
    let taken = matches!(
        &error,
        rusqlite::Error::SqliteFailure(code, _)
            if code.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_UNIQUE
                || code.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_TRIGGER
    );
    if !taken {
        return internal(error);
    }
    let table = match kind {
        NamedKind::Milestone => "issue_milestones",
        NamedKind::Label | NamedKind::LabelGroup => "issue_labels",
    };
    let stored: Result<Vec<String>, rusqlite::Error> = conn
        .prepare(&format!(
            "SELECT name FROM {table} WHERE project_id = ?1 ORDER BY id"
        ))
        .and_then(|mut statement| {
            statement
                .query_map([project_id], |row| row.get(0))?
                .collect()
        });
    let stored = match stored {
        Ok(stored) => stored,
        Err(lookup) => return internal(lookup),
    };
    match refuse_taken_name(kind, stored.iter().map(String::as_str), name) {
        Err(refusal) => refusal,
        Ok(()) => internal(error),
    }
}

fn internal(error: rusqlite::Error) -> ApiError {
    ApiError::internal(&error.to_string())
}

fn internal_storage(error: StorageError) -> ApiError {
    ApiError::internal(&error.to_string())
}

fn not_found_ticket(_: rusqlite::Error) -> ApiError {
    ApiError::not_found("issue")
}

#[cfg(test)]
mod name_case_backstop {
    use kanban_dto::ErrorCode;

    use super::SqliteIssueStore;
    use crate::db::Database;
    use crate::migrations::AllowAllMigrations;
    use crate::test_support::scratch_database;

    /// A migrated store whose project already holds the label `web` and
    /// the milestone `M1 Foundations`.
    fn store() -> (tempfile::TempDir, Database, SqliteIssueStore) {
        let (dir, mut database) = scratch_database();
        database
            .migrate(&AllowAllMigrations)
            .expect("migrations apply");
        database
            .connection()
            .execute_batch(
                "INSERT INTO projects
                     (code, name, repository, seed_workspace, default_branch,
                      herdr_session, herdr_workspace, archived, version)
                 VALUES ('CORE', 'Control plane', '/repo', '/seed', 'main',
                         'kanban-main', 'kanban.seed', 0, 1);
                 INSERT INTO issue_labels (project_id, name) VALUES (1, 'web');
                 INSERT INTO issue_milestones (project_id, name)
                 VALUES (1, 'M1 Foundations');",
            )
            .expect("the project and its names land");
        let store = SqliteIssueStore::new(&database);
        (dir, database, store)
    }

    // The application judges names before it writes; these calls skip
    // that judgement, so only the database stands in the way. It must
    // answer with the application's own refusal.

    #[test]
    fn the_store_refuses_a_label_case_variant_the_way_the_rule_does() {
        let (_dir, _database, store) = store();
        let error = store
            .create_label(1, "WEB", "#336699", None, false)
            .expect_err("the database refuses the case variant");
        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert_eq!(error.field.as_deref(), Some("name"));
        assert_eq!(
            error.message,
            "label `WEB` differs only in letter case from the existing label `web`; \
             label names are unique ignoring case"
        );

        let repeat = store
            .create_label(1, "web", "#336699", None, false)
            .expect_err("an exact repeat still conflicts");
        assert_eq!(repeat.code, ErrorCode::Conflict);
        assert_eq!(repeat.message, "label `web` already exists");
    }

    #[test]
    fn the_store_refuses_a_milestone_case_variant_the_way_the_rule_does() {
        let (_dir, _database, store) = store();
        let error = store
            .create_milestone(1, "m1 foundations", None)
            .expect_err("the database refuses the case variant");
        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert_eq!(error.field.as_deref(), Some("name"));
        assert!(
            error.message.contains("`M1 Foundations`"),
            "{}",
            error.message
        );

        let repeat = store
            .create_milestone(1, "M1 Foundations", None)
            .expect_err("an exact repeat still conflicts");
        assert_eq!(repeat.code, ErrorCode::Conflict);
    }
}
