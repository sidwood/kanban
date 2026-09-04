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
    /// A SQLite statement failed outside a named operation.
    #[error("a SQLite operation failed: {0}")]
    Sqlite(#[from] rusqlite::Error),
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
