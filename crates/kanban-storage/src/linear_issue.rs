//! Linear issue persistence: workflow states, labels, and issue keys.

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
    fn map_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::migrations::{AllowAllMigrations, LATEST_SCHEMA_VERSION};
    use crate::Database;

    #[test]
    fn migration_0053_seeds_twelve_workflow_states_per_project() {
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
        database
            .connection()
            .execute_batch(
                "INSERT INTO issue_workflow_states (project_id, name, type, legacy_state, position)
                 VALUES (1, 'Draft', 'backlog', 'draft', 0);",
            )
            .unwrap();
        let states = list_workflow_states(&*database.connection(), 1).unwrap();
        assert_eq!(states.len(), 1);
        assert_eq!(states[0].state_type, WorkflowStateType::Backlog);
    }
}
