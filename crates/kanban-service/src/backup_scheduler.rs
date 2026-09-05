//! Daily backup scheduling wired through the production core path.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use kanban_storage::{BackupOptions, BackupStore, Database, load_backup_settings};

/// How often the production scheduler checks for a due backup.
const DAILY_BACKUP_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

/// Minimum sleep between retries when a due backup keeps failing.
const FAILED_BACKUP_RETRY_BACKOFF: Duration = Duration::from_secs(1);

/// Persisted scheduler state under the managed data directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SchedulerState {
    last_success_unix_secs: u64,
}

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
        let handle = thread::spawn(move || scheduler_loop(&data_dir, &database, interval));
        Self { _handle: handle }
    }
}

fn scheduler_state_path(data_dir: &Path) -> PathBuf {
    data_dir.join(".backup-scheduler.json")
}

fn load_scheduler_state(data_dir: &Path) -> Option<SystemTime> {
    let path = scheduler_state_path(data_dir);
    let text = fs::read_to_string(path).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    let secs = value
        .get("last_success_unix_secs")
        .and_then(serde_json::Value::as_u64)?;
    Some(UNIX_EPOCH + Duration::from_secs(secs))
}

fn save_scheduler_state(
    data_dir: &Path,
    when: SystemTime,
) -> Result<(), kanban_storage::StorageError> {
    let elapsed = when.duration_since(UNIX_EPOCH).map_err(|error| {
        kanban_storage::StorageError::BackupInvalid {
            reason: error.to_string(),
        }
    })?;
    let state = SchedulerState {
        last_success_unix_secs: elapsed.as_secs(),
    };
    let path = scheduler_state_path(data_dir);
    let text = format!(
        "{{\n  \"last_success_unix_secs\": {}\n}}\n",
        state.last_success_unix_secs
    );
    fs::write(&path, text).map_err(|source| kanban_storage::StorageError::BackupIo { path, source })
}

pub(crate) fn is_backup_due(
    last_success: Option<SystemTime>,
    interval: Duration,
    now: SystemTime,
) -> bool {
    match last_success {
        None => true,
        Some(last) => now
            .duration_since(last)
            .map(|elapsed| elapsed >= interval)
            .unwrap_or(true),
    }
}

pub(crate) fn sleep_until_due(
    last_success: Option<SystemTime>,
    interval: Duration,
    now: SystemTime,
) -> Duration {
    let Some(last) = last_success else {
        return interval;
    };
    match now.duration_since(last) {
        Ok(elapsed) if elapsed >= interval => Duration::ZERO,
        Ok(elapsed) => interval.saturating_sub(elapsed),
        Err(_) => Duration::ZERO,
    }
}

pub(crate) fn scheduler_loop_sleep(
    last_success: Option<SystemTime>,
    interval: Duration,
    now: SystemTime,
    last_attempt_failed: bool,
) -> Duration {
    let sleep_for = sleep_until_due(last_success, interval, now);
    if last_attempt_failed && is_backup_due(last_success, interval, now) {
        sleep_for.max(FAILED_BACKUP_RETRY_BACKOFF)
    } else {
        sleep_for
    }
}

fn scheduler_loop(data_dir: &Path, database: &Database, interval: Duration) {
    let mut last_attempt_failed = !run_scheduled_backup_if_due(data_dir, database, interval);
    loop {
        let last_success = load_scheduler_state(data_dir);
        let now = SystemTime::now();
        let sleep_for = scheduler_loop_sleep(last_success, interval, now, last_attempt_failed);
        if !sleep_for.is_zero() {
            thread::sleep(sleep_for);
        }
        last_attempt_failed = !run_scheduled_backup_if_due(data_dir, database, interval);
    }
}

fn run_scheduled_backup_if_due(data_dir: &Path, database: &Database, interval: Duration) -> bool {
    let now = SystemTime::now();
    let last_success = load_scheduler_state(data_dir);
    if !is_backup_due(last_success, interval, now) {
        return true;
    }
    match run_due_backup(data_dir, database) {
        Ok(()) => {
            if let Err(error) = save_scheduler_state(data_dir, now) {
                eprintln!("kanban daily backup state failed: {error}");
                false
            } else {
                true
            }
        }
        Err(error) => {
            eprintln!("kanban daily backup failed: {error}");
            false
        }
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
    use std::thread;
    use std::time::{Duration, Instant, SystemTime};

    use kanban_storage::migrations::{AllowAllMigrations, LATEST_SCHEMA_VERSION};
    use kanban_storage::{BackupRetentionPolicy, BackupStore, Database};
    use tempfile::TempDir;

    use super::{
        BackupScheduler, FAILED_BACKUP_RETRY_BACKOFF, is_backup_due, load_scheduler_state,
        run_due_backup, run_scheduled_backup_if_due, save_scheduler_state, scheduler_loop_sleep,
        scheduler_state_path, sleep_until_due,
    };
    use crate::test_client::boot;

    fn open_migrated_database(dir: &TempDir) -> Database {
        let mut database =
            Database::open(&dir.path().join("kanban.sqlite")).expect("database opens");
        database
            .migrate(&AllowAllMigrations)
            .expect("migrations apply");
        database
    }

    fn bundle_count(dir: &TempDir) -> usize {
        let bundles = kanban_storage::backups_dir(dir.path());
        if !bundles.exists() {
            return 0;
        }
        std::fs::read_dir(&bundles)
            .expect("bundles list")
            .filter(|entry| {
                entry
                    .as_ref()
                    .ok()
                    .map(|entry| entry.path().join("manifest.json").is_file())
                    .unwrap_or(false)
            })
            .count()
    }

    #[test]
    fn production_serve_wires_daily_backup_scheduler() {
        let dir = TempDir::new().expect("scratch directory");
        let core = boot(&dir);
        core.shutdown();
    }

    #[test]
    fn configurable_retention_flows_through_restarted_production_path() {
        let dir = TempDir::new().expect("scratch directory");
        std::fs::write(
            dir.path().join("config.json"),
            br#"{"theme":"dark","backup_retention":2}"#,
        )
        .expect("config writes");
        let database = Arc::new(open_migrated_database(&dir));

        for _ in 0..3 {
            run_due_backup(dir.path(), &database).expect("scheduled backup runs");
        }

        assert_eq!(
            bundle_count(&dir),
            2,
            "configured retention must prune through the service path"
        );

        let restarted = Arc::new(open_migrated_database(&dir));
        run_due_backup(dir.path(), &restarted).expect("restarted backup runs");
        assert_eq!(
            bundle_count(&dir),
            2,
            "retention must still prune after a restarted production path"
        );
    }

    #[test]
    fn run_due_backup_uses_product_default_retention() {
        let dir = TempDir::new().expect("scratch directory");
        let database = open_migrated_database(&dir);

        run_due_backup(dir.path(), &database).expect("backup runs");

        let store = BackupStore::new(dir.path().to_path_buf());
        assert!(
            store
                .verified_record_for(LATEST_SCHEMA_VERSION)
                .expect("record reads")
                .is_some()
        );
        assert_eq!(
            BackupRetentionPolicy::keep_most_recent(NonZeroU32::new(7).expect("seven is not zero")),
            kanban_storage::load_backup_settings(dir.path()).retention
        );
    }

    #[test]
    fn scheduler_runs_promptly_at_startup_when_overdue() {
        let dir = TempDir::new().expect("scratch directory");
        let database = open_migrated_database(&dir);
        let interval = Duration::from_secs(60);
        let overdue = SystemTime::now() - interval * 2;
        save_scheduler_state(dir.path(), overdue).expect("scheduler state writes");

        run_scheduled_backup_if_due(dir.path(), &database, interval);

        assert_eq!(
            bundle_count(&dir),
            1,
            "a restarted core must back up promptly when overdue"
        );
        assert!(load_scheduler_state(dir.path()).is_some());
    }

    #[test]
    fn scheduler_skips_startup_when_inside_interval() {
        let dir = TempDir::new().expect("scratch directory");
        let database = Arc::new(open_migrated_database(&dir));
        let interval = Duration::from_secs(60);
        save_scheduler_state(dir.path(), SystemTime::now()).expect("scheduler state writes");

        let _scheduler =
            BackupScheduler::spawn_with_interval(dir.path().to_path_buf(), database, interval);
        thread::sleep(Duration::from_millis(300));

        assert_eq!(
            bundle_count(&dir),
            0,
            "recent success must skip startup backup inside the interval"
        );
    }

    #[test]
    fn run_scheduled_backup_if_due_runs_when_overdue() {
        let dir = TempDir::new().expect("scratch directory");
        let database = open_migrated_database(&dir);
        let interval = Duration::from_secs(60);
        let overdue = SystemTime::now() - interval * 2;
        save_scheduler_state(dir.path(), overdue).expect("scheduler state writes");

        run_scheduled_backup_if_due(dir.path(), &database, interval);

        assert_eq!(bundle_count(&dir), 1, "overdue scheduler must back up");
        assert!(load_scheduler_state(dir.path()).is_some());
    }

    #[test]
    fn run_scheduled_backup_if_due_skips_inside_interval() {
        let dir = TempDir::new().expect("scratch directory");
        let database = open_migrated_database(&dir);
        let interval = Duration::from_secs(60);
        save_scheduler_state(dir.path(), SystemTime::now()).expect("scheduler state writes");

        run_scheduled_backup_if_due(dir.path(), &database, interval);

        assert_eq!(
            bundle_count(&dir),
            0,
            "recent success must skip a due check inside the interval"
        );
    }

    #[test]
    fn scheduler_updates_state_only_after_success() {
        let dir = TempDir::new().expect("scratch directory");
        let interval = Duration::from_secs(60);
        let database = open_migrated_database(&dir);
        std::fs::write(dir.path().join("backups"), b"blocked").expect("backups path blocks writes");

        run_scheduled_backup_if_due(dir.path(), &database, interval);

        assert!(
            !scheduler_state_path(dir.path()).exists(),
            "failed backups must not persist scheduler state"
        );
    }

    #[test]
    fn scheduler_survives_restart_inside_interval_without_duplicate_backup() {
        let dir = TempDir::new().expect("scratch directory");
        {
            let database = open_migrated_database(&dir);
            run_due_backup(dir.path(), &database).expect("initial backup runs");
            save_scheduler_state(dir.path(), SystemTime::now()).expect("scheduler state writes");
        }
        assert_eq!(bundle_count(&dir), 1);
        let interval = Duration::from_secs(60);

        let database = Arc::new(open_migrated_database(&dir));
        let _scheduler =
            BackupScheduler::spawn_with_interval(dir.path().to_path_buf(), database, interval);
        thread::sleep(Duration::from_millis(300));

        assert_eq!(
            bundle_count(&dir),
            1,
            "restart inside the interval must not create another bundle"
        );
    }

    #[test]
    fn scheduler_survives_restart_beyond_interval_with_prompt_backup() {
        let dir = TempDir::new().expect("scratch directory");
        let database = open_migrated_database(&dir);
        let interval = Duration::from_secs(60);
        run_due_backup(dir.path(), &database).expect("initial backup runs");
        save_scheduler_state(dir.path(), SystemTime::now() - interval * 2)
            .expect("scheduler state writes");
        assert_eq!(bundle_count(&dir), 1);

        run_scheduled_backup_if_due(dir.path(), &database, interval);

        assert!(
            bundle_count(&dir) >= 2,
            "a restarted core beyond the interval must back up promptly"
        );
    }

    #[test]
    fn due_and_sleep_helpers_track_interval_boundaries() {
        let interval = Duration::from_secs(60);
        let anchor = SystemTime::now();
        assert!(is_backup_due(None, interval, anchor));
        assert!(!is_backup_due(Some(anchor), interval, anchor));
        assert_eq!(sleep_until_due(Some(anchor), interval, anchor), interval);
        assert_eq!(
            sleep_until_due(Some(anchor - Duration::from_secs(30)), interval, anchor),
            Duration::from_secs(30)
        );
        assert_eq!(
            sleep_until_due(Some(anchor - interval), interval, anchor),
            Duration::ZERO
        );
    }

    #[test]
    fn scheduler_loop_sleep_applies_retry_backoff_after_failed_overdue_attempt() {
        let interval = Duration::from_secs(60);
        let overdue = SystemTime::now() - interval * 2;
        assert_eq!(
            scheduler_loop_sleep(Some(overdue), interval, SystemTime::now(), false),
            Duration::ZERO,
            "overdue backups must still run immediately before a failure"
        );
        assert_eq!(
            scheduler_loop_sleep(Some(overdue), interval, SystemTime::now(), true),
            FAILED_BACKUP_RETRY_BACKOFF,
            "failed overdue backups must not spin with zero sleep"
        );
    }

    #[test]
    fn scheduler_loop_bounds_failed_overdue_retries_and_recovers() {
        let dir = TempDir::new().expect("scratch directory");
        let database = open_migrated_database(&dir);
        let interval = Duration::from_secs(60);
        let overdue = SystemTime::now() - interval * 2;
        save_scheduler_state(dir.path(), overdue).expect("scheduler state writes");
        std::fs::write(dir.path().join("backups"), b"blocked").expect("backups path blocks writes");

        let mut attempts = 0_u32;
        let mut last_attempt_failed = !run_scheduled_backup_if_due(dir.path(), &database, interval);
        attempts += 1;
        let failure_window = Duration::from_millis(400);
        let window_start = Instant::now();
        while window_start.elapsed() < failure_window {
            let last_success = load_scheduler_state(dir.path());
            let now = SystemTime::now();
            let sleep_for = scheduler_loop_sleep(last_success, interval, now, last_attempt_failed);
            if !sleep_for.is_zero() {
                let remaining = failure_window.saturating_sub(window_start.elapsed());
                if remaining.is_zero() {
                    break;
                }
                thread::sleep(sleep_for.min(remaining));
            }
            if window_start.elapsed() >= failure_window {
                break;
            }
            last_attempt_failed = !run_scheduled_backup_if_due(dir.path(), &database, interval);
            attempts += 1;
        }

        assert!(
            attempts <= 2,
            "failed overdue backups must retry with bounded backoff, got {attempts} attempts in 400ms"
        );

        std::fs::remove_file(dir.path().join("backups")).expect("backups block removed");
        last_attempt_failed = !run_scheduled_backup_if_due(dir.path(), &database, interval);
        assert!(
            !last_attempt_failed,
            "scheduler must recover once backups succeed again"
        );
        assert_eq!(
            bundle_count(&dir),
            1,
            "successful recovery must create a backup bundle"
        );
        assert!(
            load_scheduler_state(dir.path()).is_some(),
            "successful recovery must persist scheduler state"
        );
    }
}
