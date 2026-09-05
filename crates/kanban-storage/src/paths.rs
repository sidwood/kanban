//! Managed application data locations owned by the storage layer.
//!
//! The Core owns every path under `~/Library/Application
//! Support/Kanban/` (ADR-0002); the database file lives there and
//! nowhere else.

use std::path::PathBuf;

use crate::error::StorageError;

/// The root of managed application data for the Core.
pub fn managed_data_dir() -> Result<PathBuf, StorageError> {
    dirs::data_dir()
        .map(|dir| dir.join("Kanban"))
        .ok_or(StorageError::HomeUnknown)
}

/// The file name of the authoritative database inside managed
/// application data.
pub const fn database_file_name() -> &'static str {
    "kanban.sqlite"
}

/// The attachments directory inside managed application data.
pub fn attachments_dir(managed_root: &std::path::Path) -> std::path::PathBuf {
    managed_root.join("attachments")
}

/// The backups directory inside managed application data.
pub fn backups_dir(managed_root: &std::path::Path) -> std::path::PathBuf {
    managed_root.join("backups")
}

/// The structured logs directory inside managed application data.
pub fn logs_dir(managed_root: &std::path::Path) -> std::path::PathBuf {
    managed_root.join("logs")
}

/// The exported diagnostic bundles directory inside managed
/// application data.
pub fn diagnostics_dir(managed_root: &std::path::Path) -> std::path::PathBuf {
    managed_root.join("diagnostics")
}

/// The operator configuration file inside managed application data.
pub const fn config_file_name() -> &'static str {
    "config.json"
}

/// The path of the single authoritative SQLite database.
pub fn database_path() -> Result<PathBuf, StorageError> {
    Ok(managed_data_dir()?.join(database_file_name()))
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{database_path, managed_data_dir};

    #[test]
    fn managed_data_dir_sits_in_application_support() {
        let managed_dir = managed_data_dir().expect("home is known in tests");
        let components: Vec<_> = managed_dir
            .components()
            .map(|component| component.as_os_str())
            .collect();

        let tail = components
            .iter()
            .rev()
            .take(3)
            .rev()
            .cloned()
            .collect::<Vec<_>>();
        let expected: Vec<_> = ["Library", "Application Support", "Kanban"]
            .iter()
            .map(std::ffi::OsStr::new)
            .collect();
        assert_eq!(tail, expected);
    }

    #[test]
    fn database_path_names_the_sqlite_file() {
        assert_eq!(
            database_path().expect("home is known in tests"),
            managed_data_dir()
                .expect("home is known in tests")
                .join("kanban.sqlite")
        );
    }

    #[test]
    fn logs_dir_sits_in_the_managed_root() {
        assert_eq!(super::logs_dir(Path::new("/data")), Path::new("/data/logs"));
    }

    #[test]
    fn diagnostics_dir_sits_in_the_managed_root() {
        assert_eq!(
            super::diagnostics_dir(Path::new("/data")),
            Path::new("/data/diagnostics")
        );
    }
}
