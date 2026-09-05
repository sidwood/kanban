//! The local-filesystem export adapter: writes export Markdown with
//! a same-directory temporary file and an atomic rename, and reads it
//! back for drift. The adapter touches nothing but plain files — it
//! holds no git capability, so an export can never commit, push, or
//! stage anything (KAN-T35-AC3, DR-LW-13).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use kanban_app::exports::ExportFiles;
use kanban_dto::ApiError;

/// Writes and reads export files on the local filesystem.
#[derive(Debug, Default)]
pub struct LocalExportFiles;

/// Distinguishes the temporary siblings concurrent replaces create.
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

impl ExportFiles for LocalExportFiles {
    fn replace(&self, path: &Path, contents: &[u8]) -> Result<(), ApiError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|source| io_failure("create", path, source))?;
        }
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .ok_or_else(|| {
                ApiError::internal(&format!(
                    "the export path `{}` names no file",
                    path.display()
                ))
            })?;
        let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let temp = path.with_file_name(format!(".{name}.{sequence}.export-tmp"));
        if let Err(source) = std::fs::write(&temp, contents) {
            let _ = std::fs::remove_file(&temp);
            return Err(io_failure("write", &temp, source));
        }
        if let Err(source) = std::fs::rename(&temp, path) {
            let _ = std::fs::remove_file(&temp);
            return Err(io_failure("replace", path, source));
        }
        Ok(())
    }

    fn current(&self, path: &Path) -> Result<Option<Vec<u8>>, ApiError> {
        match std::fs::read(path) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(source) => Err(io_failure("read", path, source)),
        }
    }

    fn walk(&self, directory: &Path) -> Result<Vec<PathBuf>, ApiError> {
        let mut found = Vec::new();
        collect(directory, &mut found)?;
        found.sort();
        Ok(found)
    }
}

/// Recursively collect the regular files under `directory`; a
/// directory that does not exist collects nothing.
fn collect(directory: &Path, found: &mut Vec<PathBuf>) -> Result<(), ApiError> {
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(source) => return Err(io_failure("list", directory, source)),
    };
    for entry in entries.filter_map(|entry| entry.ok()) {
        let path = entry.path();
        if path.is_dir() {
            collect(&path, found)?;
        } else {
            found.push(path);
        }
    }
    Ok(())
}

/// Report a filesystem failure on one export path.
fn io_failure(verb: &str, path: &Path, source: std::io::Error) -> ApiError {
    ApiError::internal(&format!(
        "the export could not {verb} `{}`: {source}",
        path.display()
    ))
}
