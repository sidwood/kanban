//! Opening the single authoritative SQLite database in WAL mode.

use std::path::Path;

use rusqlite::Connection;

use crate::error::StorageError;
use crate::migrations::{self, MigrationReport, PreMigrationHook};
use crate::paths;

/// An open handle on the single authoritative SQLite database
/// (ADR-0002). The Core owns the only connections; nothing else,
/// including the WebView, ever opens the file directly.
pub struct Database {
    conn: Connection,
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
        Self::open(&directory.join(database_file_name()))
    }

    /// Applies the connection pragmas every connection must carry.
    fn configure(conn: Connection) -> Result<Self, StorageError> {
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        // The append-only triggers must fire for REPLACE's implicit delete.
        conn.pragma_update(None, "recursive_triggers", "ON")?;
        let database = Self { conn };
        let mode = database.journal_mode()?;
        if mode != "wal" {
            return Err(StorageError::WalRefused { mode });
        }
        Ok(database)
    }

    /// Reports the database journal mode; health surfaces use this.
    pub fn journal_mode(&self) -> Result<String, StorageError> {
        Ok(self
            .conn
            .query_row("PRAGMA journal_mode", [], |row| row.get(0))?)
    }

    /// Applies pending forward-only migrations, first offering the
    /// hook the chance to refuse (the KAN-T60 backup seam).
    pub fn migrate(
        &mut self,
        hook: &dyn PreMigrationHook,
    ) -> Result<MigrationReport, StorageError> {
        migrations::run(&mut self.conn, hook)
    }

    /// Appends to the audit trail. The only write it supports.
    pub fn append_audit_event(
        &self,
        kind: &str,
        detail: &serde_json::Value,
    ) -> Result<(), StorageError> {
        crate::audit::insert_event(&self.conn, kind, detail)
    }

    /// Appends to the activity timeline. The only write it supports.
    pub fn append_timeline_event(
        &self,
        kind: &str,
        detail: &serde_json::Value,
    ) -> Result<(), StorageError> {
        crate::timeline::insert_event(&self.conn, kind, detail)
    }

    /// The raw connection, for tests that must fabricate state.
    #[cfg(test)]
    pub(crate) fn connection(&self) -> &Connection {
        &self.conn
    }
}

/// The file name of the authoritative database inside managed
/// application data.
const fn database_file_name() -> &'static str {
    "kanban.sqlite"
}

#[cfg(test)]
mod tests {
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
