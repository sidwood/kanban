//! Managed application data locations owned by the storage layer.
//!
//! The Core owns every path under the platform data home (ADR-0002):
//! `~/Library/Application Support/Kanban/` on macOS and
//! `$XDG_DATA_HOME/Kanban` (usually `~/.local/share/Kanban`) on Linux.
//! The database file lives there and nowhere else.

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
    fn managed_data_dir_sits_under_the_platform_data_home() {
        let managed_dir = managed_data_dir().expect("home is known in tests");
        assert_eq!(
            managed_dir.file_name().and_then(|name| name.to_str()),
            Some("Kanban")
        );
        let expected = dirs::data_dir()
            .expect("the platform data home is known in tests")
            .join("Kanban");
        assert_eq!(
            managed_dir, expected,
            "managed data must sit under the platform data home"
        );
        #[cfg(target_os = "macos")]
        assert!(
            managed_dir.ends_with("Library/Application Support/Kanban"),
            "{}",
            managed_dir.display()
        );
        #[cfg(not(target_os = "macos"))]
        assert!(
            managed_dir.ends_with(".local/share/Kanban")
                || std::env::var_os("XDG_DATA_HOME")
                    .is_some_and(|home| managed_dir.starts_with(home)),
            "{}",
            managed_dir.display()
        );
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
