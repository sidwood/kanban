//! Shell preferences stored in SQLite (KAN-S5-US1, KAN-S5-US3): how
//! the operator keeps the surface arranged — the navigation rail's
//! collapse, and the board columns collapsed to their rail in each
//! scope. Per-operator data in the authoritative store, not browser
//! state: the arrangement survives a reload, a new window, and a
//! cleared browser origin. One row holds the arrangement and one
//! optimistic version guards it, so a write replaces the whole thing
//! or none of it. No row is seeded here: a shell nobody has arranged
//! reads the everyday arrangement, and the first write inserts.

use kanban_app::ShellPreferenceStore;
use kanban_domain::{BoardColumn, ProjectId, ShellPreferences, ViewScope};
use kanban_dto::ApiError;
use rusqlite::params;

use crate::db::{ConnectionHandle, Database, WriteSpan};

/// SQLite-backed shell preferences.
pub struct SqliteShellPreferenceStore {
    conn: ConnectionHandle,
}

impl SqliteShellPreferenceStore {
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

impl ShellPreferenceStore for SqliteShellPreferenceStore {
    fn preferences(&self) -> Result<ShellPreferences, ApiError> {
        let conn = self.lock();
        let row = conn.query_row(
            "SELECT rail_open, version FROM shell_preferences WHERE id = 1",
            [],
            |row| Ok((row.get::<_, i64>(0)? == 1, row.get::<_, i64>(1)? as u64)),
        );
        let (rail_open, version) = match row {
            Ok(held) => held,
            // A shell nobody has arranged holds no row.
            Err(rusqlite::Error::QueryReturnedNoRows) => return Ok(ShellPreferences::everyday()),
            Err(error) => return Err(internal(error)),
        };
        let mut statement = conn
            .prepare(
                "SELECT scope_kind, project_id, column_name
                 FROM shell_collapsed_columns
                 ORDER BY scope_kind, project_id, column_name",
            )
            .map_err(internal)?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)? as u64,
                    row.get::<_, String>(2)?,
                ))
            })
            .map_err(internal)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(internal)?;
        let mut collapsed: Vec<(ViewScope, Vec<BoardColumn>)> = Vec::new();
        for (scope_kind, project_id, column_name) in rows {
            let scope = scope_of(&scope_kind, project_id)?;
            let column = BoardColumn::parse(&column_name).ok_or_else(|| {
                ApiError::internal(&format!(
                    "stored column `{column_name}` is not a board column"
                ))
            })?;
            match collapsed.iter_mut().find(|(held, _)| *held == scope) {
                Some(entry) => entry.1.push(column),
                None => collapsed.push((scope, vec![column])),
            }
        }
        Ok(ShellPreferences::restore(rail_open, collapsed, version))
    }

    fn replace(&self, preferences: &ShellPreferences) -> Result<ShellPreferences, ApiError> {
        let conn = self.lock();
        let span = WriteSpan::begin(&conn).map_err(internal)?;
        span.execute(
            "INSERT INTO shell_preferences (id, rail_open, version)
             VALUES (1, ?1, ?2)
             ON CONFLICT (id) DO UPDATE SET rail_open = ?1, version = ?2",
            params![
                if preferences.rail_open() { 1 } else { 0 },
                preferences.version() as i64,
            ],
        )
        .map_err(internal)?;
        // The arrangement is replaced whole: a column the operator
        // expanded is a row that is gone, not a row saying so.
        span.execute("DELETE FROM shell_collapsed_columns", params![])
            .map_err(internal)?;
        for scoped in preferences.collapsed() {
            for column in scoped.columns() {
                span.execute(
                    "INSERT INTO shell_collapsed_columns (scope_kind, project_id, column_name)
                     VALUES (?1, ?2, ?3)",
                    params![
                        scope_kind_of(scoped.scope()),
                        project_column_of(scoped.scope()) as i64,
                        column.wire_name(),
                    ],
                )
                .map_err(internal)?;
            }
        }
        span.commit().map_err(internal)?;
        Ok(preferences.clone())
    }
}

fn scope_kind_of(scope: ViewScope) -> &'static str {
    match scope {
        ViewScope::Global => "global",
        ViewScope::Project(_) => "project",
    }
}

fn project_column_of(scope: ViewScope) -> u64 {
    match scope {
        ViewScope::Global => 0,
        ViewScope::Project(project) => project.value(),
    }
}

fn scope_of(scope_kind: &str, project_id: u64) -> Result<ViewScope, ApiError> {
    match scope_kind {
        "global" => Ok(ViewScope::Global),
        "project" => Ok(ViewScope::Project(ProjectId::new(project_id))),
        other => Err(ApiError::internal(&format!(
            "stored scope kind `{other}` is not a view scope"
        ))),
    }
}

fn internal(error: impl std::fmt::Display) -> ApiError {
    ApiError::internal(&error.to_string())
}

#[cfg(test)]
mod stored_shell_preferences {
    use kanban_app::ShellPreferenceStore;
    use kanban_domain::{BoardColumn, ProjectId, ShellPreferences, ViewScope};

    use super::SqliteShellPreferenceStore;
    use crate::migrations::AllowAllMigrations;
    use crate::test_support::scratch_database;

    /// A real, fully migrated scratch database: the production schema
    /// these preferences land in.
    fn migrated() -> (tempfile::TempDir, crate::db::Database) {
        let (dir, mut database) = scratch_database();
        database
            .migrate(&AllowAllMigrations)
            .expect("migrations apply");
        (dir, database)
    }

    fn project(id: u64) -> ViewScope {
        ViewScope::Project(ProjectId::new(id))
    }

    #[test]
    fn a_database_nobody_has_arranged_reads_the_everyday_arrangement() {
        let (_dir, database) = migrated();
        let store = SqliteShellPreferenceStore::new(&database);

        let held = store.preferences().expect("the preferences read");

        assert_eq!(held, ShellPreferences::everyday());
    }

    #[test]
    fn an_arrangement_written_to_the_database_is_what_a_fresh_store_reads_back() {
        let (_dir, database) = migrated();
        let store = SqliteShellPreferenceStore::new(&database);
        let arrangement = ShellPreferences::restore(
            false,
            [
                (ViewScope::Global, vec![BoardColumn::Review]),
                (project(2), vec![BoardColumn::Ready, BoardColumn::Backlog]),
            ],
            1,
        );

        store.replace(&arrangement).expect("the arrangement writes");

        // A second store over the same database is the honest stand-in
        // for the next window: nothing is carried in memory.
        let reopened = SqliteShellPreferenceStore::new(&database);
        let read = reopened.preferences().expect("the preferences read");
        assert!(!read.rail_open());
        assert_eq!(read.version(), 1);
        assert_eq!(read.collapsed_in(ViewScope::Global), &[BoardColumn::Review]);
        assert_eq!(
            read.collapsed_in(project(2)),
            &[BoardColumn::Backlog, BoardColumn::Ready]
        );
    }

    #[test]
    fn expanding_a_column_leaves_no_row_saying_it_was_collapsed() {
        let (_dir, database) = migrated();
        let store = SqliteShellPreferenceStore::new(&database);
        store
            .replace(&ShellPreferences::restore(
                true,
                [(project(2), vec![BoardColumn::Ready, BoardColumn::Backlog])],
                1,
            ))
            .expect("the arrangement writes");

        store
            .replace(&ShellPreferences::restore(
                true,
                [(project(2), vec![BoardColumn::Backlog])],
                2,
            ))
            .expect("the arrangement writes");

        let read = store.preferences().expect("the preferences read");
        assert_eq!(read.collapsed_in(project(2)), &[BoardColumn::Backlog]);
        assert_eq!(read.version(), 2);
    }

    #[test]
    fn one_scopes_arrangement_never_reaches_another() {
        let (_dir, database) = migrated();
        let store = SqliteShellPreferenceStore::new(&database);

        store
            .replace(&ShellPreferences::restore(
                true,
                [
                    (project(1), vec![BoardColumn::Review]),
                    (project(2), vec![BoardColumn::Done]),
                ],
                1,
            ))
            .expect("the arrangement writes");

        let read = store.preferences().expect("the preferences read");
        assert_eq!(read.collapsed_in(project(1)), &[BoardColumn::Review]);
        assert_eq!(read.collapsed_in(project(2)), &[BoardColumn::Done]);
        assert_eq!(read.collapsed_in(ViewScope::Global), &[]);
    }
}
