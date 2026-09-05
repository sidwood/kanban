//! Forward-only migrations from an empty schema.

use rusqlite::Connection;

use crate::db::WriteSpan;
use crate::error::StorageError;

/// One embedded, forward-only SQL migration. Versions are strictly
/// increasing and no migration is ever reverted or rewritten.
pub struct Migration {
    /// The migration version; also the file prefix.
    pub version: i64,
    /// The human name, matching the file stem.
    pub name: &'static str,
    /// The SQL applied inside one transaction.
    sql: &'static str,
}

/// A migration that is about to be applied, as seen by the
/// pre-migration hook.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PendingMigration {
    /// The migration version.
    pub version: i64,
    /// The human name.
    pub name: &'static str,
}

/// The seam a later slice uses to gate schema change; KAN-T60
/// makes it refuse to proceed without a verified backup.
pub trait PreMigrationHook {
    /// Runs once before any pending migration is applied. Returning
    /// an error aborts the run and leaves the schema untouched.
    fn before_migrate(&self, pending: &[PendingMigration]) -> Result<(), StorageError>;
}

/// The hook used until the verified-backup gate exists (KAN-T60).
pub struct AllowAllMigrations;

impl PreMigrationHook for AllowAllMigrations {
    fn before_migrate(&self, _pending: &[PendingMigration]) -> Result<(), StorageError> {
        Ok(())
    }
}

/// What a migration run did.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct MigrationReport {
    /// Versions applied by this run, in order.
    pub applied: Vec<i64>,
}

/// Every known migration, embedded at build time in strictly
/// increasing version order.
const MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        name: "initial schema",
        sql: include_str!("../migrations/0001_initial_schema.sql"),
    },
    Migration {
        version: 2,
        name: "initiatives",
        sql: include_str!("../migrations/0002_initiatives.sql"),
    },
    Migration {
        version: 3,
        name: "timeline envelope",
        sql: include_str!("../migrations/0003_timeline_envelope.sql"),
    },
    Migration {
        version: 4,
        name: "comments",
        sql: include_str!("../migrations/0004_comments.sql"),
    },
    Migration {
        version: 5,
        name: "rulings and deferrals",
        sql: include_str!("../migrations/0005_rulings_deferrals.sql"),
    },
    Migration {
        version: 6,
        name: "idempotency outcomes",
        sql: include_str!("../migrations/0006_idempotency.sql"),
    },
    Migration {
        version: 7,
        name: "timeline scope",
        sql: include_str!("../migrations/0007_timeline_scope.sql"),
    },
    Migration {
        version: 8,
        name: "evidence",
        sql: include_str!("../migrations/0008_evidence.sql"),
    },
    Migration {
        version: 9,
        name: "projects",
        sql: include_str!("../migrations/0009_projects.sql"),
    },
    Migration {
        version: 10,
        name: "herdr_settings",
        sql: include_str!("../migrations/0010_herdr_settings.sql"),
    },
];

/// Applies every pending migration, newest last, and refuses any
/// history this build does not recognise.
pub(crate) fn run(
    conn: &Connection,
    hook: &dyn PreMigrationHook,
) -> Result<MigrationReport, StorageError> {
    ensure_bookkeeping(conn)?;
    let applied = applied_versions(conn)?;
    let known = MIGRATIONS
        .iter()
        .zip(&applied)
        .all(|(migration, version)| migration.version == *version);
    if !known || applied.len() > MIGRATIONS.len() {
        return Err(StorageError::HistoryMismatch { applied });
    }

    let pending = &MIGRATIONS[applied.len()..];
    if pending.is_empty() {
        return Ok(MigrationReport::default());
    }

    let visible: Vec<PendingMigration> = pending
        .iter()
        .map(|migration| PendingMigration {
            version: migration.version,
            name: migration.name,
        })
        .collect();
    hook.before_migrate(&visible)?;

    let mut report = MigrationReport::default();
    for migration in pending {
        apply_one(conn, migration)?;
        report.applied.push(migration.version);
    }
    Ok(report)
}

/// Applies every migration up to and including `version`, for tests
/// that must fabricate the state an older schema left behind.
#[cfg(test)]
pub(crate) fn apply_through(conn: &Connection, version: i64) -> Result<(), StorageError> {
    ensure_bookkeeping(conn)?;
    for migration in MIGRATIONS.iter().filter(|entry| entry.version <= version) {
        apply_one(conn, migration)?;
    }
    Ok(())
}

/// Creates the runner's own bookkeeping table. It records state,
/// not domain history, so it exists outside the migration files.
fn ensure_bookkeeping(conn: &Connection) -> Result<(), StorageError> {
    conn.execute(
        "CREATE TABLE IF NOT EXISTS schema_migrations (
             version    INTEGER PRIMARY KEY,
             name       TEXT NOT NULL,
             applied_at TEXT NOT NULL
                 DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
         )",
        [],
    )?;
    Ok(())
}

/// The applied versions, ascending.
fn applied_versions(conn: &Connection) -> Result<Vec<i64>, StorageError> {
    let mut statement = conn.prepare("SELECT version FROM schema_migrations ORDER BY version")?;
    let versions = statement
        .query_map([], |row| row.get::<_, i64>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(versions)
}

/// Applies one migration and records it in the same write: either
/// the schema change and its bookkeeping land together or neither
/// does.
fn apply_one(conn: &Connection, migration: &Migration) -> Result<(), StorageError> {
    let span = WriteSpan::begin(conn)?;
    span.execute_batch(migration.sql)
        .map_err(|source| StorageError::Migration {
            version: migration.version,
            name: migration.name,
            source,
        })?;
    crate::audit::insert_event(
        &span,
        "migration.applied",
        &serde_json::json!({ "version": migration.version, "name": migration.name }),
    )?;
    span.execute(
        "INSERT INTO schema_migrations (version, name) VALUES (?1, ?2)",
        rusqlite::params![migration.version, migration.name],
    )?;
    span.commit()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use crate::error::StorageError;
    use crate::migrations::{
        AllowAllMigrations, MIGRATIONS, MigrationReport, PendingMigration, PreMigrationHook,
        apply_through,
    };
    use crate::test_support::scratch_database;

    /// Records every call so tests can observe hook invocations.
    struct RecordingHook {
        calls: RefCell<Vec<Vec<PendingMigration>>>,
    }

    impl PreMigrationHook for RecordingHook {
        fn before_migrate(&self, pending: &[PendingMigration]) -> Result<(), StorageError> {
            self.calls.borrow_mut().push(pending.to_vec());
            Ok(())
        }
    }

    /// Refuses to proceed, standing in for the verified-backup gate.
    struct RefusingHook;

    impl PreMigrationHook for RefusingHook {
        fn before_migrate(&self, _pending: &[PendingMigration]) -> Result<(), StorageError> {
            Err(StorageError::HookRefused {
                reason: "no verified backup".to_string(),
            })
        }
    }

    #[test]
    fn known_migrations_are_strictly_increasing() {
        for pair in MIGRATIONS.windows(2) {
            assert!(pair[0].version < pair[1].version);
        }
    }

    #[test]
    fn migrate_applies_every_known_migration_from_empty() {
        let (_dir, mut database) = scratch_database();

        let report = database
            .migrate(&AllowAllMigrations)
            .expect("the migrations apply");

        assert_eq!(
            report,
            MigrationReport {
                applied: vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10]
            }
        );
        assert_eq!(
            database
                .connection()
                .query_row("SELECT version, name FROM schema_migrations", [], |row| {
                    Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
                })
                .expect("the bookkeeping row is readable"),
            (1, "initial schema".to_string())
        );
        let versions: Vec<i64> = database
            .connection()
            .prepare("SELECT version FROM schema_migrations ORDER BY version")
            .expect("bookkeeping is readable")
            .query_map([], |row| row.get(0))
            .expect("the query runs")
            .collect::<Result<Vec<_>, _>>()
            .expect("versions decode");
        assert_eq!(versions, vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10]);
        for table in [
            "audit_events",
            "timeline_events",
            "initiatives",
            "comments",
            "comment_revisions",
            "rulings",
            "deferrals",
            "idempotency_outcomes",
            "evidence_items",
            "projects",
            "herdr_global_defaults",
            "herdr_project_settings",
        ] {
            let present: i64 = database
                .connection()
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
                    [table],
                    |row| row.get(0),
                )
                .expect("sqlite_master is readable");
            assert_eq!(present, 1, "{table} should exist");
        }
    }

    #[test]
    fn migrate_again_applies_nothing() {
        let (_dir, mut database) = scratch_database();
        database
            .migrate(&AllowAllMigrations)
            .expect("the first run applies");

        let report = database
            .migrate(&AllowAllMigrations)
            .expect("the second run succeeds");

        assert_eq!(
            report,
            MigrationReport {
                applied: Vec::new()
            }
        );
    }

    #[test]
    fn applied_migrations_are_recorded_as_audit_events() {
        let (_dir, mut database) = scratch_database();
        database
            .migrate(&AllowAllMigrations)
            .expect("the migrations apply");

        let conn = database.connection();
        let mut statement = conn
            .prepare("SELECT id, kind, detail FROM audit_events ORDER BY id")
            .expect("the audit trail is readable");
        let events: Vec<(i64, String, String)> = statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .expect("the audit query runs")
            .collect::<Result<Vec<_>, _>>()
            .expect("the audit rows decode");
        assert_eq!(events.len(), 10, "one event per applied migration");
        assert_eq!(events[0].1, "migration.applied");
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&events[0].2).expect("the detail is JSON"),
            serde_json::json!({ "version": 1, "name": "initial schema" })
        );
        assert_eq!(events[1].1, "migration.applied");
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&events[1].2).expect("the detail is JSON"),
            serde_json::json!({ "version": 2, "name": "initiatives" })
        );
        assert_eq!(events[2].1, "migration.applied");
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&events[2].2).expect("the detail is JSON"),
            serde_json::json!({ "version": 3, "name": "timeline envelope" })
        );
        assert_eq!(events[3].1, "migration.applied");
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&events[3].2).expect("the detail is JSON"),
            serde_json::json!({ "version": 4, "name": "comments" })
        );
        assert_eq!(events[4].1, "migration.applied");
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&events[4].2).expect("the detail is JSON"),
            serde_json::json!({ "version": 5, "name": "rulings and deferrals" })
        );
        assert_eq!(events[5].1, "migration.applied");
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&events[5].2).expect("the detail is JSON"),
            serde_json::json!({ "version": 6, "name": "idempotency outcomes" })
        );
        assert_eq!(events[6].1, "migration.applied");
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&events[6].2).expect("the detail is JSON"),
            serde_json::json!({ "version": 7, "name": "timeline scope" })
        );
        assert_eq!(events[7].1, "migration.applied");
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&events[7].2).expect("the detail is JSON"),
            serde_json::json!({ "version": 8, "name": "evidence" })
        );
        assert_eq!(events[8].1, "migration.applied");
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&events[8].2).expect("the detail is JSON"),
            serde_json::json!({ "version": 9, "name": "projects" })
        );
    }

    #[test]
    fn migrate_refuses_an_unrecognised_history() {
        let (_dir, mut database) = scratch_database();
        database
            .migrate(&AllowAllMigrations)
            .expect("the initial run applies");
        database
            .connection()
            .execute(
                "INSERT INTO schema_migrations (version, name) VALUES (42, 'future')",
                [],
            )
            .expect("the fabricated history lands");

        assert!(matches!(
            database.migrate(&AllowAllMigrations),
            Err(StorageError::HistoryMismatch { .. })
        ));
    }

    #[test]
    fn pre_migration_hook_observes_pending_migrations() {
        let (_dir, mut database) = scratch_database();
        let hook = RecordingHook {
            calls: RefCell::new(Vec::new()),
        };

        database.migrate(&hook).expect("the run applies");

        assert_eq!(
            hook.calls.into_inner(),
            vec![vec![
                PendingMigration {
                    version: 1,
                    name: "initial schema",
                },
                PendingMigration {
                    version: 2,
                    name: "initiatives",
                },
                PendingMigration {
                    version: 3,
                    name: "timeline envelope",
                },
                PendingMigration {
                    version: 4,
                    name: "comments",
                },
                PendingMigration {
                    version: 5,
                    name: "rulings and deferrals",
                },
                PendingMigration {
                    version: 6,
                    name: "idempotency outcomes",
                },
                PendingMigration {
                    version: 7,
                    name: "timeline scope",
                },
                PendingMigration {
                    version: 8,
                    name: "evidence",
                },
                PendingMigration {
                    version: 9,
                    name: "projects",
                },
                PendingMigration {
                    version: 10,
                    name: "herdr_settings",
                },
            ]]
        );
    }

    #[test]
    fn pre_migration_hook_refusal_leaves_the_schema_untouched() {
        let (_dir, mut database) = scratch_database();

        let outcome = database.migrate(&RefusingHook);

        assert!(matches!(outcome, Err(StorageError::HookRefused { .. })));
        let table_present: i64 = database
            .connection()
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'timeline_events'",
                [],
                |row| row.get(0),
            )
            .expect("sqlite_master is readable");
        assert_eq!(table_present, 0, "no migration SQL may have run");
    }

    #[test]
    fn migration_0010_backfills_herdr_settings_for_existing_projects() {
        let (_dir, mut database) = scratch_database();
        apply_through(&database.connection(), 9).expect("the pre-herdr schema applies");
        database
            .connection()
            .execute(
                "INSERT INTO projects
                     (code, name, repository, seed_workspace, default_branch,
                      herdr_session, archived, version)
                 VALUES ('CORE', 'Control plane', '/repositories/kanban',
                         '/workspaces/kanban.seed', 'main', 'kanban-main', 0, 1)",
                [],
            )
            .expect("the pre-upgrade Project lands");

        let report = database
            .migrate(&AllowAllMigrations)
            .expect("migration 0010 applies");

        assert_eq!(report.applied, vec![10]);
        let settings: (i64, i64, i64, i64, i64) = database
            .connection()
            .query_row(
                "SELECT reconciliation_interval_secs, polling_fallback_enabled,
                        polling_fallback_interval_secs, stall_deadline_secs,
                        missing_result_deadline_secs
                 FROM herdr_project_settings WHERE project_id = 1",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )
            .expect("the upgraded Project inherits settings");
        assert_eq!(
            settings,
            (300, 0, 10, 3600, 7200),
            "every pre-existing Project copies the global defaults"
        );
    }

    #[test]
    fn migration_0010_leaves_fresh_databases_without_project_settings_rows() {
        let (_dir, mut database) = scratch_database();

        database
            .migrate(&AllowAllMigrations)
            .expect("the migrations apply");

        let count: i64 = database
            .connection()
            .query_row("SELECT COUNT(*) FROM herdr_project_settings", [], |row| {
                row.get(0)
            })
            .expect("project settings are readable");
        assert_eq!(
            count, 0,
            "a fresh database creates tables but seeds settings only at registration"
        );
    }

    #[test]
    fn pre_migration_hook_is_skipped_when_nothing_is_pending() {
        let (_dir, mut database) = scratch_database();
        database
            .migrate(&AllowAllMigrations)
            .expect("the first run applies");
        let hook = RecordingHook {
            calls: RefCell::new(Vec::new()),
        };

        database.migrate(&hook).expect("the second run succeeds");

        assert!(hook.calls.into_inner().is_empty());
    }
}
