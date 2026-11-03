//! SQLite-backed Linear issue fields and history.

use kanban_app::TimelineEnvelope;
use kanban_domain::{IssueKey, IssueStateTimestamps, TicketState};
use kanban_dto::{ApiError, IssueRelationRecord};
use rusqlite::{params, OptionalExtension};

use crate::db::{ConnectionHandle, Database, WriteSpan};
use crate::error::StorageError;
use crate::timeline::insert_event;

use super::linear_issue::{workflow_state_by_legacy, workflow_state_by_name, WorkflowStateRecord};

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
        conn.execute(
            "UPDATE tickets SET description = ?2, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') WHERE id = ?1",
            params![ticket_id, description],
        )
        .map_err(internal)?;
        Ok(())
    }

    pub fn patch_description(
        &self,
        ticket_id: i64,
        patch: &str,
    ) -> Result<String, ApiError> {
        let conn = self.lock();
        let current: String = conn
            .query_row(
                "SELECT description FROM tickets WHERE id = ?1",
                [ticket_id],
                |row| row.get(0),
            )
            .map_err(not_found_ticket)?;
        let merged = format!("{current}{patch}");
        conn.execute(
            "UPDATE tickets SET description = ?2, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') WHERE id = ?1",
            params![ticket_id, &merged],
        )
        .map_err(internal)?;
        Ok(merged)
    }

    pub fn set_linear_priority(&self, ticket_id: i64, priority: u8) -> Result<(), ApiError> {
        let conn = self.lock();
        conn.execute(
            "UPDATE tickets SET linear_priority = ?2, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') WHERE id = ?1",
            params![ticket_id, i64::from(priority)],
        )
        .map_err(internal)?;
        Ok(())
    }

    pub fn set_due_date(&self, ticket_id: i64, due_date: Option<&str>) -> Result<(), ApiError> {
        let conn = self.lock();
        let stored = due_date.unwrap_or("2099-12-31");
        conn.execute(
            "UPDATE tickets SET due_date = ?2, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') WHERE id = ?1",
            params![ticket_id, stored],
        )
        .map_err(internal)?;
        Ok(())
    }

    pub fn set_milestone(&self, ticket_id: i64, milestone_id: Option<i64>) -> Result<(), ApiError> {
        let conn = self.lock();
        conn.execute(
            "UPDATE tickets SET milestone_id = ?2, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') WHERE id = ?1",
            params![ticket_id, milestone_id],
        )
        .map_err(internal)?;
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
        let span = WriteSpan::begin(&conn).map_err(internal)?;
        let now: String = span
            .query_row("SELECT strftime('%Y-%m-%dT%H:%M:%fZ', 'now')", [], |row| row.get(0))
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
        let legacy = TicketState::parse(&to_state.legacy_state).ok_or_else(|| {
            ApiError::internal("stored workflow state is invalid")
        })?;
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
        let _ = project_id;
        span.commit().map_err(internal)?;
        Ok(())
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

    pub fn add_label(
        &self,
        project_id: i64,
        ticket_id: i64,
        name: &str,
    ) -> Result<(), ApiError> {
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
            return Err(ApiError::invalid_request(&format!("unknown label `{name}`")));
        };
        if retired_at.is_some() {
            return Err(ApiError::invalid_request(&format!(
                "label `{name}` is retired"
            )));
        }
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
        if let Some((group_id, single)) = group
            && single == 1
        {
            conn.execute(
                "DELETE FROM issue_ticket_labels
                 WHERE ticket_id = ?1
                   AND label_id IN (
                       SELECT l.id FROM issue_labels AS l
                       WHERE l.group_id = ?2
                   )",
                params![ticket_id, group_id],
            )
            .map_err(internal)?;
        }
        conn.execute(
            "INSERT OR IGNORE INTO issue_ticket_labels (ticket_id, label_id) VALUES (?1, ?2)",
            params![ticket_id, label_id],
        )
        .map_err(internal)?;
        self.touch_updated_at(ticket_id)
    }

    pub fn remove_label(&self, project_id: i64, ticket_id: i64, name: &str) -> Result<(), ApiError> {
        let conn = self.lock();
        conn.execute(
            "DELETE FROM issue_ticket_labels
             WHERE ticket_id = ?1
               AND label_id = (
                   SELECT id FROM issue_labels WHERE project_id = ?2 AND name = ?3
               )",
            params![ticket_id, project_id, name],
        )
        .map_err(internal)?;
        self.touch_updated_at(ticket_id)
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

    pub fn list_project_labels(&self, project_id: i64) -> Result<Vec<kanban_app::IssueLabelRow>, ApiError> {
        let conn = self.lock();
        let mut statement = conn
            .prepare(
                "SELECT l.name, l.color, g.name, l.retired_at IS NOT NULL
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
                    retired: row.get(3)?,
                })
            })
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
                "SELECT id, name, target_date FROM issue_milestones
                 WHERE project_id = ?1 ORDER BY sort_order, name",
            )
            .map_err(internal)?;
        statement
            .query_map([project_id], |row| {
                Ok(kanban_app::IssueMilestoneRow {
                    id: row.get::<_, i64>(0)? as u64,
                    name: row.get(1)?,
                    target_date: row.get(2)?,
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
            "SELECT description, linear_priority, due_date, milestone_id, updated_at
             FROM tickets WHERE id = ?1",
            [ticket_id],
            |row| {
                Ok(kanban_app::IssueTicketFields {
                    description: row.get(0)?,
                    linear_priority: row.get::<_, i64>(1)? as u8,
                    due_date: row.get(2)?,
                    milestone_id: row.get(3)?,
                    updated_at: row.get(4)?,
                })
            },
        )
        .map_err(not_found_ticket)
    }

    pub fn list_relations(&self, ticket_id: i64) -> Result<Vec<IssueRelationRecord>, ApiError> {
        let conn = self.lock();
        let mut statement = conn
            .prepare(
                "SELECT 'blocks' AS kind, p.code, t.number
                 FROM issue_relations AS r
                 JOIN tickets AS t ON t.id = r.to_ticket_id
                 JOIN projects AS p ON p.id = t.project_id
                 WHERE r.from_ticket_id = ?1 AND r.kind = 'blocks'
                 UNION ALL
                 SELECT 'blocked-by', p.code, t.number
                 FROM issue_relations AS r
                 JOIN tickets AS t ON t.id = r.from_ticket_id
                 JOIN projects AS p ON p.id = t.project_id
                 WHERE r.to_ticket_id = ?1 AND r.kind = 'blocks'
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
        rows.collect::<Result<Vec<_>, _>>().map_err(internal)
    }
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
        SqliteIssueStore::workflow_state_for_ticket(self, ticket_id).map(|state| {
            state.map(map_workflow_state)
        })
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
}

pub fn resolve_state_by_name(
    conn: &rusqlite::Connection,
    project_id: i64,
    name: &str,
) -> Result<WorkflowStateRecord, ApiError> {
    workflow_state_by_name(conn, project_id, name)
        .map_err(internal_storage)?
        .ok_or_else(|| ApiError::invalid_request(&format!("unknown state `{name}`")))
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
