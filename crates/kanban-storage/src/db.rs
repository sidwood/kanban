//! Opening the single authoritative SQLite database in WAL mode.

use std::path::Path;
use std::sync::Arc;

use parking_lot::{ReentrantMutex, ReentrantMutexGuard};
use rusqlite::Connection;

use crate::error::StorageError;
use crate::migrations::{self, MigrationReport, PreMigrationHook};
use crate::paths;
use crate::timeline::{TimelineFilter, TimelineRow};

/// The shareable handle to the one connection the core owns.
/// rusqlite connections are `Send` but not `Sync`; the lock is what
/// lets storage-backed command handlers serve across transport
/// threads through the same connection.
///
/// The lock is re-entrant because a mutation must hold the
/// connection from its first row to the commit that records its
/// replay outcome, while the handlers running inside that span
/// write through their own handles on the same connection. A plain
/// mutex would deadlock on that second acquisition; releasing it
/// between writes would let another thread read rows the span may
/// still discard.
pub(crate) type ConnectionHandle = Arc<ReentrantMutex<Connection>>;

/// Open an atomic write on `conn`, nesting inside any write already
/// open on it. Spans nest strictly last in, first out, so one
/// savepoint name serves every depth.
pub(crate) fn open_span(conn: &Connection) -> Result<(), rusqlite::Error> {
    conn.execute_batch("SAVEPOINT kanban_write_span")
}

/// Land the innermost open write on `conn`. Releasing the outermost
/// span is what commits the transaction SQLite started under it.
pub(crate) fn land_span(conn: &Connection) -> Result<(), rusqlite::Error> {
    conn.execute_batch("RELEASE kanban_write_span")
}

/// Discard the innermost open write on `conn`. Rolling back to a
/// savepoint leaves it open, so the release must follow: an
/// abandoned savepoint would hold the transaction open against every
/// later writer. Nothing useful remains to be done with a failure
/// here, because the caller is already unwinding an error and the
/// next writer reports the open span itself.
pub(crate) fn discard_span(conn: &Connection) {
    let _ = conn.execute_batch("ROLLBACK TO kanban_write_span; RELEASE kanban_write_span");
}

/// One atomic write on the shared connection. The outermost span is
/// the transaction SQLite commits; spans opened inside it are
/// savepoints, so an aggregate's rows, its timeline appends, and the
/// replay outcome recorded against them land together or not at all.
/// A span dropped without [`WriteSpan::commit`] discards everything
/// written inside it, including the spans it held.
pub(crate) struct WriteSpan<'a> {
    conn: &'a Connection,
    committed: bool,
}

impl<'a> WriteSpan<'a> {
    /// Open a span on `conn`.
    pub(crate) fn begin(conn: &'a Connection) -> Result<Self, rusqlite::Error> {
        open_span(conn)?;
        Ok(Self {
            conn,
            committed: false,
        })
    }

    /// Land everything written inside the span.
    pub(crate) fn commit(mut self) -> Result<(), rusqlite::Error> {
        land_span(self.conn)?;
        self.committed = true;
        Ok(())
    }
}

impl std::ops::Deref for WriteSpan<'_> {
    type Target = Connection;

    fn deref(&self) -> &Connection {
        self.conn
    }
}

impl Drop for WriteSpan<'_> {
    fn drop(&mut self) {
        if !self.committed {
            discard_span(self.conn);
        }
    }
}

/// An open handle on the single authoritative SQLite database
/// (ADR-0002). The Core owns the only connections; nothing else,
/// including the WebView, ever opens the file directly.
pub struct Database {
    conn: ConnectionHandle,
}

impl Database {
    /// Opens (creating if needed) the database at `path` in WAL
    /// journal mode. The parent directory must already exist.
    pub fn open(path: &Path) -> Result<Self, StorageError> {
        let conn = Connection::open(path).map_err(|source| StorageError::Open {
            path: path.to_path_buf(),
            source,
        })?;
        Self::configure(conn)
    }

    /// Opens the managed database, creating managed application data
    /// on first boot.
    pub fn open_managed() -> Result<Self, StorageError> {
        Self::open_in(&paths::managed_data_dir()?)
    }

    /// Creates `directory` if needed and opens `kanban.sqlite`
    /// inside it.
    fn open_in(directory: &Path) -> Result<Self, StorageError> {
        std::fs::create_dir_all(directory).map_err(|source| StorageError::ManagedDir {
            path: directory.to_path_buf(),
            source,
        })?;
        Self::open(&directory.join(paths::database_file_name()))
    }

    /// Applies the connection pragmas every connection must carry.
    fn configure(conn: Connection) -> Result<Self, StorageError> {
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        // The append-only triggers must fire for REPLACE's implicit delete.
        conn.pragma_update(None, "recursive_triggers", "ON")?;
        let database = Self {
            conn: Arc::new(ReentrantMutex::new(conn)),
        };
        let mode = database.journal_mode()?;
        if mode != "wal" {
            return Err(StorageError::WalRefused { mode });
        }
        Ok(database)
    }

    /// The shareable handle to the underlying connection, for
    /// storage-backed ports serving the application core.
    pub(crate) fn connection_handle(&self) -> ConnectionHandle {
        self.conn.clone()
    }

    /// Reports the database journal mode; health surfaces use this.
    pub fn journal_mode(&self) -> Result<String, StorageError> {
        let conn = self.lock();
        Ok(conn.query_row("PRAGMA journal_mode", [], |row| row.get(0))?)
    }

    /// Reports the applied schema version, or 0 before any migration
    /// has run; health surfaces use this.
    pub fn schema_version(&self) -> Result<i64, StorageError> {
        let conn = self.lock();
        Ok(conn.query_row(
            "SELECT COALESCE(MAX(version), 0) FROM schema_migrations",
            [],
            |row| row.get(0),
        )?)
    }

    /// When the newest timeline row was recorded — the database's own
    /// last-change time. The stored format orders lexicographically,
    /// so the maximum is the newest; absent when nothing is recorded
    /// yet.
    pub fn last_change_at(&self) -> Result<Option<String>, StorageError> {
        let conn = self.lock();
        Ok(
            conn.query_row("SELECT MAX(recorded_at) FROM timeline_events", [], |row| {
                row.get(0)
            })?,
        )
    }

    /// When the newest Workspace timeline row was recorded; absent
    /// when no Workspace has changed yet.
    pub fn last_workspace_change_at(&self) -> Result<Option<String>, StorageError> {
        let conn = self.lock();
        Ok(conn.query_row(
            "SELECT MAX(recorded_at) FROM timeline_events WHERE entity_kind = ?1",
            [kanban_dto::TimelineEntityKind::Workspace.as_str()],
            |row| row.get(0),
        )?)
    }

    /// Applies pending forward-only migrations, first offering the
    /// hook the chance to refuse (the KAN-T60 backup seam).
    pub fn migrate(
        &mut self,
        hook: &dyn PreMigrationHook,
    ) -> Result<MigrationReport, StorageError> {
        let conn = self.lock();
        migrations::run(&conn, hook)
    }

    /// Appends to the audit trail. The only write it supports.
    pub fn append_audit_event(
        &self,
        kind: &str,
        detail: &serde_json::Value,
    ) -> Result<(), StorageError> {
        crate::audit::insert_event(&self.lock(), kind, detail)
    }

    /// Appends to the activity timeline. The only write it supports.
    pub fn append_timeline_event(
        &self,
        event: &kanban_app::TimelineEnvelope,
    ) -> Result<(), StorageError> {
        crate::timeline::insert_event(&self.lock(), event)
    }

    /// Reads timeline rows for `filter`, oldest first.
    pub fn query_timeline(
        &self,
        filter: &TimelineFilter,
    ) -> Result<Vec<TimelineRow>, StorageError> {
        crate::timeline::query_events(&self.lock(), filter)
    }

    /// Lock the connection; every internal writer goes through here.
    fn lock(&self) -> ReentrantMutexGuard<'_, Connection> {
        self.conn.lock()
    }

    /// The raw connection, for tests that must fabricate state.
    #[cfg(test)]
    pub(crate) fn connection(&self) -> ReentrantMutexGuard<'_, Connection> {
        self.lock()
    }
}

#[cfg(test)]
mod tests {
    use rusqlite::Connection;
    use tempfile::TempDir;

    use super::Database;
    use crate::error::StorageError;

    fn scratch_dir() -> TempDir {
        tempfile::tempdir().expect("a scratch directory is available")
    }

    #[test]
    fn open_enables_wal_journal_mode() {
        let dir = scratch_dir();
        let database = Database::open(&dir.path().join("kanban.sqlite"))
            .expect("opening a fresh database succeeds");

        assert_eq!(
            database.journal_mode().expect("journal mode is readable"),
            "wal"
        );
    }

    #[test]
    fn open_enables_recursive_triggers() {
        let dir = scratch_dir();
        let database = Database::open(&dir.path().join("kanban.sqlite"))
            .expect("opening a fresh database succeeds");

        let recursive: i64 = database
            .connection()
            .query_row("PRAGMA recursive_triggers", [], |row| row.get(0))
            .expect("recursive triggers are readable");
        assert_eq!(recursive, 1);
    }

    #[test]
    fn open_refuses_a_missing_parent_directory() {
        let dir = scratch_dir();
        let path = dir.path().join("missing").join("kanban.sqlite");

        assert!(matches!(
            Database::open(&path),
            Err(StorageError::Open { .. })
        ));
    }

    #[test]
    fn wal_journal_mode_persists_across_reopens() {
        let dir = scratch_dir();
        let path = dir.path().join("kanban.sqlite");

        drop(Database::open(&path).expect("the first open succeeds"));
        let reopened = Database::open(&path).expect("the second open succeeds");

        assert_eq!(
            reopened.journal_mode().expect("journal mode is readable"),
            "wal"
        );
    }

    #[test]
    fn schema_version_reports_the_applied_migrations() {
        let dir = scratch_dir();
        let mut database =
            Database::open(&dir.path().join("kanban.sqlite")).expect("the database opens");
        assert_eq!(
            database.schema_version().expect("the version reads"),
            0,
            "an unmigrated database has applied nothing"
        );

        database
            .migrate(&crate::migrations::AllowAllMigrations)
            .expect("the migrations apply");

        assert_eq!(
            database.schema_version().expect("the version reads"),
            crate::migrations::LATEST_SCHEMA_VERSION
        );
    }

    #[test]
    fn last_change_at_tracks_the_newest_recorded_row() {
        use kanban_app::TimelineEnvelope;
        use kanban_dto::{TimelineEntityKind, TimelineEntityRef, TimelineEventKind};
        use serde_json::json;

        let (_dir, mut database) = crate::test_support::scratch_database();
        database
            .migrate(&crate::migrations::AllowAllMigrations)
            .expect("the migrations apply");

        assert_eq!(
            database.last_change_at().expect("the change reads"),
            None,
            "nothing is recorded yet"
        );
        assert_eq!(
            database
                .last_workspace_change_at()
                .expect("the change reads"),
            None,
            "no Workspace has changed yet"
        );

        database
            .append_timeline_event(&TimelineEnvelope::project(
                1,
                TimelineEventKind::Transition,
                Some(TimelineEntityRef {
                    kind: TimelineEntityKind::Ticket,
                    id: "kan-t1".to_owned(),
                }),
                json!({ "action": "created" }),
            ))
            .expect("the row lands");
        database
            .append_timeline_event(&TimelineEnvelope::project(
                1,
                TimelineEventKind::Transition,
                Some(TimelineEntityRef {
                    kind: TimelineEntityKind::Workspace,
                    id: "3".to_owned(),
                }),
                json!({ "action": "registered" }),
            ))
            .expect("the row lands");

        let overall = database
            .last_change_at()
            .expect("the change reads")
            .expect("a recorded row exists");
        let workspace = database
            .last_workspace_change_at()
            .expect("the change reads")
            .expect("a Workspace row exists");
        assert!(
            overall.starts_with("20"),
            "the change is a stored timestamp: {overall}"
        );
        assert_eq!(
            workspace, overall,
            "the Workspace row is the newest row of all"
        );
    }

    #[test]
    fn a_write_span_lands_every_write_made_inside_it() {
        let (_dir, mut database) = crate::test_support::scratch_database();
        database
            .migrate(&crate::migrations::AllowAllMigrations)
            .expect("the migrations apply");
        let store_handle = database.connection_handle();

        {
            let conn = database.connection();
            let span = super::WriteSpan::begin(&conn).expect("the span opens");
            insert_initiative(&span, "Outer");
            // A second handle, as every store holds one, writes
            // inside the span the first handle opened.
            let nested_conn = store_handle.lock();
            let nested = super::WriteSpan::begin(&nested_conn).expect("the nested span opens");
            insert_initiative(&nested, "Nested");
            nested.commit().expect("the nested span lands");
            span.commit().expect("the holding span lands");
        }

        assert_eq!(
            initiative_names(&database),
            vec!["Outer".to_owned(), "Nested".to_owned()]
        );
    }

    #[test]
    fn dropping_a_write_span_discards_the_writes_nested_inside_it() {
        let (_dir, mut database) = crate::test_support::scratch_database();
        database
            .migrate(&crate::migrations::AllowAllMigrations)
            .expect("the migrations apply");
        let store_handle = database.connection_handle();

        {
            let conn = database.connection();
            let span = super::WriteSpan::begin(&conn).expect("the span opens");
            insert_initiative(&span, "Outer");
            let nested_conn = store_handle.lock();
            let nested = super::WriteSpan::begin(&nested_conn).expect("the nested span opens");
            insert_initiative(&nested, "Nested");
            nested.commit().expect("the nested span lands");
            drop(span);
        }

        assert!(
            initiative_names(&database).is_empty(),
            "a nested write cannot outlive the span that holds it"
        );
    }

    #[test]
    fn the_connection_is_writable_again_after_a_span_is_discarded() {
        let (_dir, mut database) = crate::test_support::scratch_database();
        database
            .migrate(&crate::migrations::AllowAllMigrations)
            .expect("the migrations apply");

        {
            let conn = database.connection();
            drop(super::WriteSpan::begin(&conn).expect("the span opens"));
        }
        {
            let conn = database.connection();
            let span = super::WriteSpan::begin(&conn).expect("the next span opens");
            insert_initiative(&span, "After");
            span.commit().expect("the next span lands");
        }

        assert_eq!(initiative_names(&database), vec!["After".to_owned()]);
    }

    /// Write one Initiative row through `conn`.
    fn insert_initiative(conn: &Connection, name: &str) {
        conn.execute(
            "INSERT INTO initiatives (name, archived, version) VALUES (?1, 0, 1)",
            [name],
        )
        .expect("the row inserts");
    }

    /// Every stored Initiative name, in insertion order.
    fn initiative_names(database: &Database) -> Vec<String> {
        let conn = database.connection();
        let mut statement = conn
            .prepare("SELECT name FROM initiatives ORDER BY id")
            .expect("the initiatives table is readable");
        statement
            .query_map([], |row| row.get(0))
            .expect("the query runs")
            .collect::<Result<Vec<_>, _>>()
            .expect("the names decode")
    }

    #[test]
    fn open_in_creates_the_containing_directory() {
        let dir = scratch_dir();
        let managed = dir.path().join("Kanban");

        let database = Database::open_in(&managed).expect("open_in succeeds");

        assert!(managed.join("kanban.sqlite").exists());
        assert_eq!(
            database.journal_mode().expect("journal mode is readable"),
            "wal"
        );
    }
}
