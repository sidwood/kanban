//! Shared test fixtures: every storage test runs against a real
//! SQLite file in a scratch directory (docs/architecture/
//! verification.md).

use tempfile::TempDir;

use crate::db::Database;

/// A real database in a fresh temporary directory.
pub(crate) fn scratch_database() -> (TempDir, Database) {
    let dir = tempfile::tempdir().expect("a scratch directory is available");
    let database =
        Database::open(&dir.path().join("kanban.sqlite")).expect("a scratch database opens");
    (dir, database)
}
