//! Herdr observation settings stored in SQLite (KAN-S8).

use kanban_app::HerdrSettingsStore;
use kanban_dto::{
    ApiError, HerdrDefaultsUpdateRequest, HerdrGlobalDefaults, HerdrProjectSettings,
    HerdrSettingsUpdateRequest,
};
use rusqlite::params;

use crate::db::{ConnectionHandle, Database, WriteSpan};

/// SQLite-backed Herdr settings.
pub struct SqliteHerdrSettingsStore {
    conn: ConnectionHandle,
}

impl SqliteHerdrSettingsStore {
    /// Share the connection the `database` owns.
    pub fn new(database: &Database) -> Self {
        Self {
            conn: database.connection_handle(),
        }
    }

    fn lock(&self) -> parking_lot::ReentrantMutexGuard<'_, rusqlite::Connection> {
        self.conn.lock()
    }
}

impl HerdrSettingsStore for SqliteHerdrSettingsStore {
    fn global_defaults(&self) -> Result<HerdrGlobalDefaults, ApiError> {
        let conn = self.lock();
        conn.query_row(
            "SELECT reconciliation_interval_secs, stall_deadline_secs,
                    missing_result_deadline_secs, version
             FROM herdr_global_defaults WHERE id = 1",
            [],
            |row| {
                Ok(HerdrGlobalDefaults {
                    reconciliation_interval_secs: row.get::<_, i64>(0)? as u64,
                    stall_deadline_secs: row.get::<_, i64>(1)? as u64,
                    missing_result_deadline_secs: row.get::<_, i64>(2)? as u64,
                    version: row.get::<_, i64>(3)? as u64,
                })
            },
        )
        .map_err(internal)
    }

    fn update_global_defaults(
        &self,
        request: &HerdrDefaultsUpdateRequest,
    ) -> Result<HerdrGlobalDefaults, ApiError> {
        let conn = self.lock();
        let span = WriteSpan::begin(&conn).map_err(internal)?;
        let expected = request.mutation.optimistic_version;
        let changed = span
            .execute(
                "UPDATE herdr_global_defaults
                 SET reconciliation_interval_secs = ?1,
                     stall_deadline_secs = ?2,
                     missing_result_deadline_secs = ?3,
                     version = version + 1
                 WHERE id = 1 AND version = ?4",
                params![
                    request.reconciliation_interval_secs as i64,
                    request.stall_deadline_secs as i64,
                    request.missing_result_deadline_secs as i64,
                    expected as i64,
                ],
            )
            .map_err(internal)?;
        if changed != 1 {
            let current = span.query_row(
                "SELECT version FROM herdr_global_defaults WHERE id = 1",
                [],
                |row| row.get::<_, i64>(0),
            );
            return match current {
                Ok(current) => Err(ApiError::stale_version(expected, current as u64)),
                Err(rusqlite::Error::QueryReturnedNoRows) => {
                    Err(ApiError::not_found("herdr defaults"))
                }
                Err(error) => Err(internal(error)),
            };
        }
        span.commit().map_err(internal)?;
        self.global_defaults()
    }

    fn project_settings(&self, project_id: u64) -> Result<HerdrProjectSettings, ApiError> {
        let conn = self.lock();
        conn.query_row(
            "SELECT reconciliation_interval_secs, polling_fallback_enabled,
                    polling_fallback_interval_secs, stall_deadline_secs,
                    missing_result_deadline_secs, version
             FROM herdr_project_settings WHERE project_id = ?1",
            params![project_id as i64],
            decode_settings_row,
        )
        .map_err(|error| match error {
            rusqlite::Error::QueryReturnedNoRows => {
                ApiError::not_found(&format!("herdr settings for project {project_id}"))
            }
            other => internal(other),
        })
    }

    fn update_project_settings(
        &self,
        request: &HerdrSettingsUpdateRequest,
    ) -> Result<HerdrProjectSettings, ApiError> {
        let conn = self.lock();
        let span = WriteSpan::begin(&conn).map_err(internal)?;
        let expected = request.mutation.optimistic_version;
        let changed = span
            .execute(
                "UPDATE herdr_project_settings
                 SET reconciliation_interval_secs = ?2,
                     polling_fallback_enabled = ?3,
                     polling_fallback_interval_secs = ?4,
                     stall_deadline_secs = ?5,
                     missing_result_deadline_secs = ?6,
                     version = version + 1
                 WHERE project_id = ?1 AND version = ?7",
                params![
                    request.project_id as i64,
                    request.reconciliation_interval_secs as i64,
                    request.polling_fallback_enabled,
                    request.polling_fallback_interval_secs as i64,
                    request.stall_deadline_secs as i64,
                    request.missing_result_deadline_secs as i64,
                    expected as i64,
                ],
            )
            .map_err(internal)?;
        if changed != 1 {
            let current = span.query_row(
                "SELECT version FROM herdr_project_settings WHERE project_id = ?1",
                params![request.project_id as i64],
                |row| row.get::<_, i64>(0),
            );
            return match current {
                Ok(current) => Err(ApiError::stale_version(expected, current as u64)),
                Err(rusqlite::Error::QueryReturnedNoRows) => Err(ApiError::not_found(&format!(
                    "project {}",
                    request.project_id
                ))),
                Err(error) => Err(internal(error)),
            };
        }
        span.commit().map_err(internal)?;
        self.project_settings(request.project_id)
    }

    fn seed_project_settings(&self, project_id: u64) -> Result<(), ApiError> {
        let defaults = self.global_defaults()?;
        let conn = self.lock();
        conn.execute(
            "INSERT INTO herdr_project_settings
                 (project_id, reconciliation_interval_secs, polling_fallback_enabled,
                  polling_fallback_interval_secs, stall_deadline_secs,
                  missing_result_deadline_secs, version)
             VALUES (?1, ?2, 0, 10, ?3, ?4, 1)",
            params![
                project_id as i64,
                defaults.reconciliation_interval_secs as i64,
                defaults.stall_deadline_secs as i64,
                defaults.missing_result_deadline_secs as i64,
            ],
        )
        .map_err(internal)?;
        Ok(())
    }
}

fn decode_settings_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<HerdrProjectSettings> {
    Ok(HerdrProjectSettings {
        reconciliation_interval_secs: row.get::<_, i64>(0)? as u64,
        polling_fallback_enabled: row.get::<_, i64>(1)? != 0,
        polling_fallback_interval_secs: row.get::<_, i64>(2)? as u64,
        stall_deadline_secs: row.get::<_, i64>(3)? as u64,
        missing_result_deadline_secs: row.get::<_, i64>(4)? as u64,
        version: row.get::<_, i64>(5)? as u64,
    })
}

fn internal(error: impl ToString) -> ApiError {
    ApiError::internal(&error.to_string())
}

#[cfg(test)]
mod tests {
    use kanban_dto::HerdrSettingsUpdateRequest;

    use super::SqliteHerdrSettingsStore;
    use crate::migrations::AllowAllMigrations;
    use crate::{Database, SqliteProjectStore};
    use kanban_app::HerdrSettingsStore;
    use kanban_app::ProjectStore;
    use kanban_domain::ProjectRegistration;

    fn database() -> Database {
        let dir = tempfile::tempdir().expect("a scratch directory is available");
        let mut database = Database::open(&dir.path().join("kanban.sqlite"))
            .expect("opening a fresh database succeeds");
        database
            .migrate(&AllowAllMigrations)
            .expect("the migrations apply");
        database
    }

    fn register_project(database: &Database, code: &str, session: &str) -> u64 {
        let store = SqliteProjectStore::new(database);
        let herdr = SqliteHerdrSettingsStore::new(database);
        let registration = ProjectRegistration::new(
            code,
            "Control plane",
            "/repositories/kanban",
            "/workspaces/kanban.seed",
            "main",
            session,
            None,
        )
        .expect("the registration validates");
        let project = store
            .create(&registration, &|_| {
                kanban_app::TimelineEnvelope::project(
                    "1",
                    kanban_dto::TimelineEventKind::Transition,
                    None,
                    serde_json::json!({ "action": "registered" }),
                )
                .expect("the envelope validates")
            })
            .expect("the project registers");
        herdr
            .seed_project_settings(project.id().value())
            .expect("settings seed from defaults");
        project.id().value()
    }

    #[test]
    fn seeding_copies_global_defaults_for_a_new_project() {
        let database = database();
        let store = SqliteHerdrSettingsStore::new(&database);
        let project_id = register_project(&database, "CORE", "kanban-main");

        let settings = store
            .project_settings(project_id)
            .expect("settings exist after seeding");

        assert_eq!(settings.reconciliation_interval_secs, 300);
        assert!(!settings.polling_fallback_enabled);
        assert_eq!(settings.polling_fallback_interval_secs, 10);
        assert_eq!(settings.stall_deadline_secs, 3600);
        assert_eq!(settings.missing_result_deadline_secs, 7200);
    }

    #[test]
    fn updating_project_settings_requires_the_current_version() {
        let database = database();
        let store = SqliteHerdrSettingsStore::new(&database);
        let project_id = register_project(&database, "WAVE", "wave-main");
        let current = store
            .project_settings(project_id)
            .expect("settings exist after seeding");

        let updated = store
            .update_project_settings(&HerdrSettingsUpdateRequest {
                mutation: kanban_dto::MutationContext {
                    optimistic_version: current.version,
                    idempotency_key: "herdr-settings-1".to_owned(),
                },
                project_id,
                reconciliation_interval_secs: 600,
                polling_fallback_enabled: true,
                polling_fallback_interval_secs: 10,
                stall_deadline_secs: 1800,
                missing_result_deadline_secs: 3600,
            })
            .expect("the update lands");

        assert_eq!(updated.reconciliation_interval_secs, 600);
        assert!(updated.polling_fallback_enabled);
        assert_eq!(updated.version, current.version + 1);
    }
}
