//! The storage error type.

use std::path::PathBuf;

/// Everything that can go wrong inside the storage layer.
#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    /// The user home directory cannot be determined, so managed
    /// application data has no location.
    #[error("the user home directory cannot be determined")]
    HomeUnknown,
    /// Managed application data could not be created.
    #[error("managed application data at {path} is unusable: {source}")]
    ManagedDir {
        /// The managed application data directory.
        path: PathBuf,
        /// The underlying filesystem error.
        source: std::io::Error,
    },
    /// The database file could not be opened or created.
    #[error("opening the database at {path} failed: {source}")]
    Open {
        /// The database file that was requested.
        path: PathBuf,
        /// The underlying SQLite failure.
        source: rusqlite::Error,
    },
    /// SQLite refused to switch the database into WAL journal mode.
    #[error("the database refused WAL journal mode (reported {mode:?})")]
    WalRefused {
        /// The journal mode SQLite reported instead.
        mode: String,
    },
    /// The pre-migration hook refused to let the run proceed.
    #[error("the pre-migration hook refused: {reason}")]
    HookRefused {
        /// Why the hook refused.
        reason: String,
    },
    /// The database holds applied migrations this build does not
    /// recognise as the start of its own list.
    #[error("applied migration history {applied:?} does not extend this build's known migrations")]
    HistoryMismatch {
        /// The applied versions, ascending, at the failure.
        applied: Vec<i64>,
    },
    /// A migration's SQL failed.
    #[error("migration {version} ({name}) failed: {source}")]
    Migration {
        /// The failing migration version.
        version: i64,
        /// The failing migration name.
        name: &'static str,
        /// The underlying SQLite failure.
        source: rusqlite::Error,
    },
    /// A SQLite statement failed outside a named operation.
    #[error("a SQLite operation failed: {0}")]
    Sqlite(#[from] rusqlite::Error),
    /// A timeline write or query carried invalid inputs.
    #[error("invalid timeline input: {reason}")]
    InvalidTimeline {
        /// Why the input was rejected.
        reason: String,
    },
    /// A backup bundle could not be read or written.
    #[error("backup I/O at {path:?} failed: {source}")]
    BackupIo {
        /// The path that failed.
        path: PathBuf,
        /// The underlying filesystem error.
        source: std::io::Error,
    },
    /// A backup snapshot could not be opened.
    #[error("opening backup snapshot at {path:?} failed: {source}")]
    BackupOpen {
        /// The snapshot path.
        path: PathBuf,
        /// The underlying SQLite failure.
        source: rusqlite::Error,
    },
    /// A backup manifest or payload was invalid.
    #[error("invalid backup bundle: {reason}")]
    BackupInvalid {
        /// Why the bundle was rejected.
        reason: String,
    },
    /// A backup file hash did not match its manifest.
    #[error("backup hash mismatch for {path}: expected {expected}, got {actual}")]
    BackupHashMismatch {
        /// The manifest path entry.
        path: String,
        /// The hash recorded in the manifest.
        expected: String,
        /// The hash recomputed from disk.
        actual: String,
    },
    /// A backup file size did not match its manifest.
    #[error("backup size mismatch for {path}: expected {expected}, got {actual}")]
    BackupSizeMismatch {
        /// The manifest path entry.
        path: String,
        /// The size recorded in the manifest.
        expected: u64,
        /// The size read from disk.
        actual: u64,
    },
    /// A backup database failed integrity checks.
    #[error("backup database integrity check failed: {detail}")]
    BackupIntegrity {
        /// The integrity check output.
        detail: String,
    },
}

#[cfg(test)]
mod tests {
    use super::StorageError;

    #[test]
    fn home_unknown_renders_a_stable_message() {
        assert_eq!(
            StorageError::HomeUnknown.to_string(),
            "the user home directory cannot be determined"
        );
    }
}
