//! Linear issue persistence: workflow states, labels, and issue keys.

use crate::db::WriteSpan;
use crate::error::StorageError;
use kanban_domain::{IssueKey, LinearPriority, WorkflowStateType};
use rusqlite::{Connection, OptionalExtension};

/// One workflow state row for a project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowStateRecord {
    pub id: i64,
    pub project_id: i64,
    pub name: String,
    pub state_type: WorkflowStateType,
    pub legacy_state: String,
    pub position: i64,
}

impl WorkflowStateRecord {
    pub(crate) fn map_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        let state_type: String = row.get(3)?;
        Ok(Self {
            id: row.get(0)?,
            project_id: row.get(1)?,
            name: row.get(2)?,
            state_type: WorkflowStateType::parse(&state_type).expect("stored state type"),
            legacy_state: row.get(4)?,
            position: row.get(5)?,
        })
    }
}

pub fn list_workflow_states(
    conn: &Connection,
    project_id: i64,
) -> Result<Vec<WorkflowStateRecord>, StorageError> {
    let mut statement = conn
        .prepare(
            "SELECT id, project_id, name, type, legacy_state, position
             FROM issue_workflow_states
             WHERE project_id = ?1
             ORDER BY position",
        )
        .map_err(StorageError::from)?;
    let rows = statement
        .query_map([project_id], WorkflowStateRecord::map_row)
        .map_err(StorageError::from)?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(StorageError::from)
}

pub fn resolve_issue_key(
    conn: &Connection,
    identifier: &str,
) -> Result<Option<(IssueKey, i64)>, StorageError> {
    let key = match IssueKey::parse(identifier) {
        Some(key) => key,
        None => return Ok(None),
    };
    let code = key.project_code().as_str();
    let number = key.number().value();
    let ticket_id: Option<i64> = conn
        .query_row(
            "SELECT t.id FROM tickets AS t
             JOIN projects AS p ON p.id = t.project_id
             WHERE p.code = ?1 AND t.number = ?2",
            rusqlite::params![code, i64::try_from(number).unwrap_or(i64::MAX)],
            |row| row.get(0),
        )
        .optional()
        .map_err(StorageError::from)?;
    Ok(ticket_id.map(|id| (key, id)))
}

pub fn linear_priority_for_ticket(
    conn: &Connection,
    ticket_id: i64,
) -> Result<Option<LinearPriority>, StorageError> {
    let value: Option<i64> = conn
        .query_row(
            "SELECT linear_priority FROM tickets WHERE id = ?1",
            [ticket_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(StorageError::from)?;
    Ok(value.and_then(|v| LinearPriority::new(v as u8)))
}

/// Whether Linear issue tables from migration 0053 are present.
pub(crate) fn issue_schema_ready(conn: &Connection) -> bool {
    conn.query_row(
        "SELECT 1 FROM sqlite_master
         WHERE type = 'table' AND name = 'issue_workflow_states' LIMIT 1",
        [],
        |_| Ok(()),
    )
    .is_ok()
}

const DEFAULT_WORKFLOW: &[(&str, &str, &str, i64)] = &[
    ("Draft", "backlog", "draft", 0),
    ("Parked", "backlog", "parked", 1),
    ("Blocked", "backlog", "blocked", 2),
    ("Scheduled", "unstarted", "scheduled", 3),
    ("Ready", "unstarted", "ready", 4),
    ("Active", "started", "active", 5),
    ("In Review", "started", "in_review", 6),
    ("Approved", "started", "approved", 7),
    ("Landing", "started", "landing", 8),
    ("Done", "completed", "done", 9),
    ("Cancelled", "canceled", "cancelled", 10),
    ("Superseded", "canceled", "superseded", 11),
    ("Duplicate", "duplicate", "duplicate", 12),
];

/// Seed default workflow, Type labels, and label group for a new project.
pub(crate) fn seed_project_defaults(
    span: &WriteSpan<'_>,
    project_id: i64,
) -> Result<(), StorageError> {
    for (name, kind, legacy, position) in DEFAULT_WORKFLOW {
        span.execute(
            "INSERT OR IGNORE INTO issue_workflow_states
                 (project_id, name, type, legacy_state, position)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![project_id, name, kind, legacy, position],
        )
        .map_err(StorageError::from)?;
    }
    span.execute(
        "INSERT OR IGNORE INTO issue_label_groups (project_id, name, single_select)
         VALUES (?1, 'Type', 1)",
        [project_id],
    )
    .map_err(StorageError::from)?;
    for (name, color) in [
        ("implementation", "#6366F1"),
        ("bug", "#EF4444"),
        ("task", "#10B981"),
    ] {
        span.execute(
            "INSERT OR IGNORE INTO issue_labels (project_id, group_id, name, color)
             SELECT ?1, g.id, ?2, ?3
             FROM issue_label_groups AS g
             WHERE g.project_id = ?1 AND g.name = 'Type'",
            rusqlite::params![project_id, name, color],
        )
        .map_err(StorageError::from)?;
    }
    Ok(())
}

pub fn seed_project_defaults_on_connection(
    conn: &Connection,
    project_id: i64,
) -> Result<(), StorageError> {
    let span = WriteSpan::begin(conn).map_err(StorageError::from)?;
    seed_project_defaults(&span, project_id)?;
    span.commit().map_err(StorageError::from)?;
    Ok(())
}

pub fn workflow_state_by_name(
    conn: &Connection,
    project_id: i64,
    name: &str,
) -> Result<Option<WorkflowStateRecord>, StorageError> {
    let mut statement = conn
        .prepare(
            "SELECT id, project_id, name, type, legacy_state, position
             FROM issue_workflow_states
             WHERE project_id = ?1 AND name = ?2",
        )
        .map_err(StorageError::from)?;
    statement
        .query_row(
            rusqlite::params![project_id, name],
            WorkflowStateRecord::map_row,
        )
        .optional()
        .map_err(StorageError::from)
}

pub fn workflow_state_by_legacy(
    conn: &Connection,
    project_id: i64,
    legacy_state: &str,
) -> Result<Option<WorkflowStateRecord>, StorageError> {
    let mut statement = conn
        .prepare(
            "SELECT id, project_id, name, type, legacy_state, position
             FROM issue_workflow_states
             WHERE project_id = ?1 AND legacy_state = ?2",
        )
        .map_err(StorageError::from)?;
    statement
        .query_row(
            rusqlite::params![project_id, legacy_state],
            WorkflowStateRecord::map_row,
        )
        .optional()
        .map_err(StorageError::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Database;
    use crate::migrations::{AllowAllMigrations, LATEST_SCHEMA_VERSION};

    #[test]
    fn migration_0053_seeds_thirteen_workflow_states_per_project() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("kanban.sqlite");
        let mut database = Database::open(&path).unwrap();
        database.migrate(&AllowAllMigrations).unwrap();
        assert_eq!(
            database.schema_version().unwrap(),
            LATEST_SCHEMA_VERSION,
            "expected schema {LATEST_SCHEMA_VERSION}"
        );
        database
            .connection()
            .execute(
                "INSERT INTO projects
                 (code, name, repository, seed_workspace, default_branch,
                  herdr_session, herdr_workspace, archived, version)
                 VALUES ('CORE', 'Control plane', '/repo', '/seed', 'main',
                         'kanban-main', 'kanban.seed', 0, 1)",
                [],
            )
            .unwrap();
        // Re-run is a no-op; seed only runs in migration. Insert states manually
        // for a project created after migration 53 on empty DB — production
        // registers projects before tickets; this asserts the table exists.
        let count: i64 = database
            .connection()
            .query_row(
                "SELECT COUNT(*) FROM issue_workflow_states WHERE project_id = 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 0);
        crate::linear_issue::seed_project_defaults_on_connection(&database.connection(), 1)
            .unwrap();
        let count: i64 = database
            .connection()
            .query_row(
                "SELECT COUNT(*) FROM issue_workflow_states WHERE project_id = 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 13);
        let states = list_workflow_states(&database.connection(), 1).unwrap();
        assert_eq!(states.len(), 13);
        assert_eq!(states[0].state_type, WorkflowStateType::Backlog);
    }
}
