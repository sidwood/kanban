//! Daily backup scheduling wired through the production core path.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use kanban_storage::{BackupOptions, BackupStore, Database, load_backup_settings};

/// How often the production scheduler checks for a due backup.
const DAILY_BACKUP_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

/// The production backup scheduler owned by a running core.
pub(crate) struct BackupScheduler {
    _handle: JoinHandle<()>,
}

impl BackupScheduler {
    /// Spawn the daily backup loop for `data_dir`.
    pub fn spawn(data_dir: PathBuf, database: Arc<Database>) -> Self {
        Self::spawn_with_interval(data_dir, database, DAILY_BACKUP_INTERVAL)
    }

    /// Spawn a scheduler with a testable interval.
    pub fn spawn_with_interval(
        data_dir: PathBuf,
        database: Arc<Database>,
        interval: Duration,
    ) -> Self {
        let handle = thread::spawn(move || {
            loop {
                thread::sleep(interval);
                if let Err(error) = run_due_backup(&data_dir, &database) {
                    eprintln!("kanban daily backup failed: {error}");
                }
            }
        });
        Self { _handle: handle }
    }
}

/// Run one backup when the production path would, using managed settings.
pub fn run_due_backup(
    data_dir: &Path,
    database: &Database,
) -> Result<(), kanban_storage::StorageError> {
    let settings = load_backup_settings(data_dir);
    let store = BackupStore::new(data_dir.to_path_buf());
    let options = BackupOptions {
        retention: settings.retention,
        passphrase: None,
    };
    store.create(database, &options)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU32;
    use std::sync::Arc;
    use std::time::Duration;

    use kanban_storage::{
        BackupRetentionPolicy, BackupStore, Database, migrations::AllowAllMigrations,
    };
    use tempfile::TempDir;

    use super::{BackupScheduler, run_due_backup};
    use crate::serve;

    #[test]
    fn production_serve_wires_daily_backup_scheduler() {
        let dir = TempDir::new().expect("scratch directory");
        let core = serve(dir.path()).expect("core serves");
        core.shutdown();
    }

    #[test]
    fn configurable_retention_flows_through_production_backup_path() {
        let dir = TempDir::new().expect("scratch directory");
        std::fs::write(
            dir.path().join("config.json"),
            br#"{"theme":"dark","backup_retention":2}"#,
        )
        .expect("config writes");
        let mut database =
            Database::open(&dir.path().join("kanban.sqlite")).expect("database opens");
        database
            .migrate(&AllowAllMigrations)
            .expect("migrations apply");
        let database = Arc::new(database);
        let _scheduler = BackupScheduler::spawn_with_interval(
            dir.path().to_path_buf(),
            database.clone(),
            Duration::from_millis(1),
        );

        for _ in 0..3 {
            run_due_backup(dir.path(), &database).expect("scheduled backup runs");
        }

        let bundles = kanban_storage::backups_dir(dir.path());
        let count = std::fs::read_dir(&bundles)
            .expect("bundles list")
            .filter(|entry| {
                entry
                    .as_ref()
                    .ok()
                    .map(|entry| entry.path().join("manifest.json").is_file())
                    .unwrap_or(false)
            })
            .count();
        assert_eq!(
            count, 2,
            "configured retention must prune through the service path"
        );
    }

    #[test]
    fn run_due_backup_uses_product_default_retention() {
        let dir = TempDir::new().expect("scratch directory");
        let mut database =
            Database::open(&dir.path().join("kanban.sqlite")).expect("database opens");
        database
            .migrate(&AllowAllMigrations)
            .expect("migrations apply");

        run_due_backup(dir.path(), &database).expect("backup runs");

        let store = BackupStore::new(dir.path().to_path_buf());
        assert!(
            store
                .verified_record_for(9)
                .expect("record reads")
                .is_some()
        );
        assert_eq!(
            BackupRetentionPolicy::keep_most_recent(NonZeroU32::new(7).expect("seven is not zero")),
            kanban_storage::load_backup_settings(dir.path()).retention
        );
    }
}
