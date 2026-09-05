//! The SQLite implementation of the Workspace storage port.

use kanban_app::{TimelineEnvelope, WorkspaceStore, duplicate_path_error};
use kanban_domain::{
    ProjectId, Workspace, WorkspaceHealth, WorkspaceId, WorkspaceObservation, WorkspaceRegistration,
};
use kanban_dto::ApiError;
use rusqlite::params;

use crate::db::{ConnectionHandle, Database, WriteSpan};
use crate::timeline::insert_event;

const WORKSPACE_COLUMNS: &str = "id, project_id, path, is_seed, retired, lane_id, health, \
                                 repository_identity, branch, head, working_tree_clean, version";

/// The Workspace port over the authoritative database.
pub struct SqliteWorkspaceStore {
    conn: ConnectionHandle,
}

impl SqliteWorkspaceStore {
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

impl WorkspaceStore for SqliteWorkspaceStore {
    fn create(
        &self,
        registration: &WorkspaceRegistration,
        envelope: &dyn Fn(WorkspaceId) -> TimelineEnvelope,
    ) -> Result<Workspace, ApiError> {
        let conn = self.lock();
        let span = WriteSpan::begin(&conn).map_err(internal)?;
        if path_holder(&span, registration.project_id(), registration.path())?.is_some() {
            return Err(duplicate_path_error(registration.path()));
        }
        span.execute(
            "INSERT INTO workspaces
                 (project_id, path, is_seed, retired, lane_id, health, version)
             VALUES (?1, ?2, ?3, 0, NULL, 'missing', 1)",
            params![
                registration.project_id().value() as i64,
                registration.path(),
                registration.is_seed(),
            ],
        )
        .map_err(internal)?;
        let id = WorkspaceId::new(
            span.last_insert_rowid()
                .try_into()
                .map_err(|_| ApiError::internal("the Workspace identity overflowed"))?,
        );
        append_timeline(&span, &envelope(id))?;
        span.commit().map_err(internal)?;
        Ok(Workspace::new(id, registration.clone()))
    }

    fn find(&self, id: WorkspaceId) -> Result<Option<Workspace>, ApiError> {
        let conn = self.lock();
        let row = conn.query_row(
            &format!("SELECT {WORKSPACE_COLUMNS} FROM workspaces WHERE id = ?1"),
            params![id.value() as i64],
            decode_row,
        );
        match row {
            Ok(workspace) => Ok(Some(workspace)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(error) => Err(internal(error)),
        }
    }

    fn save(&self, workspace: &Workspace, envelope: TimelineEnvelope) -> Result<(), ApiError> {
        let conn = self.lock();
        let span = WriteSpan::begin(&conn).map_err(internal)?;
        let observation = workspace.observation();
        let preceding_version = workspace.version() - 1;
        let changed = span
            .execute(
                "UPDATE workspaces
                 SET retired = ?2,
                     lane_id = ?3,
                     health = ?4,
                     repository_identity = ?5,
                     branch = ?6,
                     head = ?7,
                     working_tree_clean = ?8,
                     version = ?9
                 WHERE id = ?1 AND version = ?10",
                params![
                    workspace.id().value() as i64,
                    workspace.is_retired(),
                    workspace.lane_id().map(|id| id as i64),
                    workspace.health().as_str(),
                    observation.repository_identity(),
                    observation.branch(),
                    observation.head(),
                    observation.working_tree_clean().map(|clean| clean as i64),
                    workspace.version() as i64,
                    preceding_version as i64,
                ],
            )
            .map_err(internal)?;
        if changed != 1 {
            let current = span.query_row(
                "SELECT version FROM workspaces WHERE id = ?1",
                params![workspace.id().value() as i64],
                |row| row.get::<_, i64>(0),
            );
            return match current {
                Ok(current) => Err(ApiError::stale_version(
                    preceding_version,
                    current.unsigned_abs(),
                )),
                Err(rusqlite::Error::QueryReturnedNoRows) => Err(ApiError::not_found(&format!(
                    "workspace {}",
                    workspace.id()
                ))),
                Err(error) => Err(internal(error)),
            };
        }
        append_timeline(&span, &envelope)?;
        span.commit().map_err(internal)?;
        Ok(())
    }

    fn list_for_project(&self, project_id: ProjectId) -> Result<Vec<Workspace>, ApiError> {
        let conn = self.lock();
        let mut statement = conn
            .prepare(&format!(
                "SELECT {WORKSPACE_COLUMNS}
                 FROM workspaces
                 WHERE project_id = ?1
                 ORDER BY id"
            ))
            .map_err(internal)?;
        let rows = statement
            .query_map(params![project_id.value() as i64], decode_row)
            .map_err(internal)?;
        let mut workspaces = Vec::new();
        for row in rows {
            workspaces.push(row.map_err(internal)?);
        }
        Ok(workspaces)
    }

    fn path_taken(&self, project_id: ProjectId, path: &str) -> Result<bool, ApiError> {
        let conn = self.lock();
        Ok(path_holder(&conn, project_id, path)?.is_some())
    }
}

fn path_holder(
    conn: &rusqlite::Connection,
    project_id: ProjectId,
    path: &str,
) -> Result<Option<i64>, ApiError> {
    match conn.query_row(
        "SELECT id FROM workspaces WHERE project_id = ?1 AND path = ?2",
        params![project_id.value() as i64, path],
        |row| row.get(0),
    ) {
        Ok(id) => Ok(Some(id)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(error) => Err(internal(error)),
    }
}

fn decode_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Workspace> {
    let health = row.get::<_, String>(6)?;
    let health = WorkspaceHealth::parse(&health).ok_or_else(|| {
        rusqlite::Error::InvalidColumnType(6, "health".to_owned(), rusqlite::types::Type::Text)
    })?;
    let lane_id = row.get::<_, Option<i64>>(5)?.map(|id| id as u64);
    let working_tree_clean = match row.get::<_, Option<i64>>(10)? {
        Some(1) => Some(true),
        Some(0) => Some(false),
        None => None,
        _ => {
            return Err(rusqlite::Error::InvalidColumnType(
                10,
                "working_tree_clean".to_owned(),
                rusqlite::types::Type::Integer,
            ));
        }
    };
    let mut observation = WorkspaceObservation::empty();
    if row.get::<_, Option<String>>(7)?.is_some()
        || row.get::<_, Option<String>>(8)?.is_some()
        || row.get::<_, Option<String>>(9)?.is_some()
        || working_tree_clean.is_some()
    {
        observation.apply_git_read(
            row.get(7)?,
            row.get(8)?,
            row.get(9)?,
            working_tree_clean.unwrap_or(true),
            lane_id,
        );
    } else {
        observation.clear_git_read(lane_id);
    }
    let registration = WorkspaceRegistration::new(
        ProjectId::new(row.get::<_, i64>(1)? as u64),
        &row.get::<_, String>(2)?,
        row.get::<_, i64>(3)? == 1,
    )
    .map_err(|_| {
        rusqlite::Error::InvalidColumnType(2, "path".to_owned(), rusqlite::types::Type::Text)
    })?;
    Ok(Workspace::restore(
        WorkspaceId::new(row.get::<_, i64>(0)? as u64),
        registration,
        row.get::<_, i64>(4)? == 1,
        lane_id,
        health,
        observation,
        row.get::<_, i64>(11)? as u64,
    ))
}

fn append_timeline(span: &WriteSpan<'_>, envelope: &TimelineEnvelope) -> Result<(), ApiError> {
    insert_event(span, envelope).map_err(internal)
}

fn internal(error: impl std::error::Error) -> ApiError {
    ApiError::internal(&error.to_string())
}
