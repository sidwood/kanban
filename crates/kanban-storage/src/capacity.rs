//! Capacity settings stored in SQLite (KAN-S7): the global defaults
//! row the migration seeds, and one optional row per Project holding
//! the stricter caps it imposes. A Project that never set caps holds
//! no row: reads answer unset caps at version 1 and the first update
//! inserts, so absence stays the honest record of a Project that
//! imposes nothing.

use kanban_app::CapacityStore;
use kanban_dto::{
    ApiError, CapacityDefaultsUpdateRequest, CapacityGlobalDefaults, CapacityProjectCaps,
    CapacitySettingsUpdateRequest,
};
use rusqlite::params;

use crate::db::{ConnectionHandle, Database, WriteSpan};

/// SQLite-backed capacity settings.
pub struct SqliteCapacityStore {
    conn: ConnectionHandle,
}

impl SqliteCapacityStore {
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

impl CapacityStore for SqliteCapacityStore {
    fn global_defaults(&self) -> Result<CapacityGlobalDefaults, ApiError> {
        let conn = self.lock();
        conn.query_row(
            "SELECT max_active_per_harness, max_active_per_model,
                    max_active_per_usage_pool, version
             FROM capacity_global_defaults WHERE id = 1",
            [],
            |row| {
                Ok(CapacityGlobalDefaults {
                    max_active_per_harness: row.get::<_, i64>(0)? as u64,
                    max_active_per_model: row.get::<_, i64>(1)? as u64,
                    max_active_per_usage_pool: row.get::<_, i64>(2)? as u64,
                    version: row.get::<_, i64>(3)? as u64,
                })
            },
        )
        .map_err(internal)
    }

    fn update_global_defaults(
        &self,
        request: &CapacityDefaultsUpdateRequest,
    ) -> Result<CapacityGlobalDefaults, ApiError> {
        let conn = self.lock();
        let span = WriteSpan::begin(&conn).map_err(internal)?;
        let expected = request.mutation.optimistic_version;
        let changed = span
            .execute(
                "UPDATE capacity_global_defaults
                 SET max_active_per_harness = ?1,
                     max_active_per_model = ?2,
                     max_active_per_usage_pool = ?3,
                     version = version + 1
                 WHERE id = 1 AND version = ?4",
                params![
                    request.max_active_per_harness as i64,
                    request.max_active_per_model as i64,
                    request.max_active_per_usage_pool as i64,
                    expected as i64,
                ],
            )
            .map_err(internal)?;
        if changed != 1 {
            let current = span.query_row(
                "SELECT version FROM capacity_global_defaults WHERE id = 1",
                [],
                |row| row.get::<_, i64>(0),
            );
            return match current {
                Ok(current) => Err(ApiError::stale_version(expected, current as u64)),
                Err(rusqlite::Error::QueryReturnedNoRows) => {
                    Err(ApiError::not_found("capacity defaults"))
                }
                Err(error) => Err(internal(error)),
            };
        }
        span.commit().map_err(internal)?;
        self.global_defaults()
    }

    fn project_caps(&self, project_id: u64) -> Result<CapacityProjectCaps, ApiError> {
        let conn = self.lock();
        match conn.query_row(
            "SELECT max_active_per_harness, max_active_per_model,
                    max_active_per_usage_pool, max_active_lanes, version
             FROM capacity_project_caps WHERE project_id = ?1",
            params![project_id as i64],
            decode_caps_row,
        ) {
            Ok(caps) => Ok(caps),
            // Absence is a Project that imposes nothing, reported as
            // unset caps at version 1 so the first update inserts.
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(CapacityProjectCaps {
                max_active_per_harness: None,
                max_active_per_model: None,
                max_active_per_usage_pool: None,
                max_active_lanes: None,
                version: 1,
            }),
            Err(other) => Err(internal(other)),
        }
    }

    fn update_project_caps(
        &self,
        request: &CapacitySettingsUpdateRequest,
    ) -> Result<CapacityProjectCaps, ApiError> {
        let conn = self.lock();
        let span = WriteSpan::begin(&conn).map_err(internal)?;
        let expected = request.mutation.optimistic_version;
        let stored: Option<i64> = span
            .query_row(
                "SELECT version FROM capacity_project_caps WHERE project_id = ?1",
                params![request.project_id as i64],
                |row| row.get::<_, i64>(0),
            )
            .map(Some)
            .or_else(|error| match error {
                rusqlite::Error::QueryReturnedNoRows => Ok(None),
                other => Err(internal(other)),
            })?;
        let changed = match stored {
            Some(current) if current as u64 == expected => span
                .execute(
                    "UPDATE capacity_project_caps
                     SET max_active_per_harness = ?2,
                         max_active_per_model = ?3,
                         max_active_per_usage_pool = ?4,
                         max_active_lanes = ?5,
                         version = version + 1
                     WHERE project_id = ?1 AND version = ?6",
                    params![
                        request.project_id as i64,
                        request.max_active_per_harness.map(|v| v as i64),
                        request.max_active_per_model.map(|v| v as i64),
                        request.max_active_per_usage_pool.map(|v| v as i64),
                        request.max_active_lanes.map(|v| v as i64),
                        current,
                    ],
                )
                .map_err(internal)?,
            // The row is absent and the caller held the unset
            // version the read path reports, so this is the first
            // caps a Project imposes.
            None if expected == 1 => span
                .execute(
                    "INSERT INTO capacity_project_caps
                         (project_id, max_active_per_harness, max_active_per_model,
                          max_active_per_usage_pool, max_active_lanes, version)
                     VALUES (?1, ?2, ?3, ?4, ?5, 2)",
                    params![
                        request.project_id as i64,
                        request.max_active_per_harness.map(|v| v as i64),
                        request.max_active_per_model.map(|v| v as i64),
                        request.max_active_per_usage_pool.map(|v| v as i64),
                        request.max_active_lanes.map(|v| v as i64),
                    ],
                )
                .map_err(internal)?,
            Some(current) => {
                return Err(ApiError::stale_version(expected, current as u64));
            }
            None => {
                return Err(ApiError::stale_version(expected, 1));
            }
        };
        if changed != 1 {
            return Err(ApiError::internal("the capacity caps write changed no row"));
        }
        span.commit().map_err(internal)?;
        self.project_caps(request.project_id)
    }
}

fn decode_caps_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<CapacityProjectCaps> {
    Ok(CapacityProjectCaps {
        max_active_per_harness: row.get::<_, Option<i64>>(0)?.map(|v| v as u64),
        max_active_per_model: row.get::<_, Option<i64>>(1)?.map(|v| v as u64),
        max_active_per_usage_pool: row.get::<_, Option<i64>>(2)?.map(|v| v as u64),
        max_active_lanes: row.get::<_, Option<i64>>(3)?.map(|v| v as u64),
        version: row.get::<_, i64>(4)? as u64,
    })
}

fn internal(error: impl ToString) -> ApiError {
    ApiError::internal(&error.to_string())
}

#[cfg(test)]
mod tests {
    use kanban_app::{CapacityStore, ProjectStore};
    use kanban_dto::MutationContext;

    use super::SqliteCapacityStore;
    use crate::migrations::AllowAllMigrations;
    use crate::{Database, SqliteProjectStore};
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

    fn register_project(database: &Database, code: &str) -> u64 {
        let store = SqliteProjectStore::new(database);
        let registration = ProjectRegistration::new(
            code,
            "Control plane",
            "/repositories/kanban",
            "/workspaces/kanban.seed",
            "main",
            "kanban.seed",
            Some("kanban-main"),
            None,
        )
        .expect("the registration validates");
        let project = store
            .create(&registration, &|_| {
                kanban_app::TimelineEnvelope::project(
                    1,
                    kanban_dto::TimelineEventKind::Transition,
                    None,
                    serde_json::json!({ "action": "registered" }),
                )
            })
            .expect("the project registers");
        project.id().value()
    }

    fn mutation(version: u64) -> MutationContext {
        MutationContext {
            optimistic_version: version,
            idempotency_key: "capacity-test".to_owned(),
        }
    }

    #[test]
    fn the_seeded_defaults_serve_and_update_under_their_version() {
        let database = database();
        let store = SqliteCapacityStore::new(&database);

        let seeded = store.global_defaults().expect("the defaults serve");
        assert_eq!(seeded.max_active_per_harness, 2);
        assert_eq!(seeded.max_active_per_model, 2);
        assert_eq!(seeded.max_active_per_usage_pool, 4);
        assert_eq!(seeded.version, 1);

        let updated = store
            .update_global_defaults(&kanban_dto::CapacityDefaultsUpdateRequest {
                mutation: mutation(1),
                max_active_per_harness: 3,
                max_active_per_model: 1,
                max_active_per_usage_pool: 5,
            })
            .expect("the update lands");
        assert_eq!(updated.max_active_per_harness, 3);
        assert_eq!(updated.version, 2);

        let refused = store
            .update_global_defaults(&kanban_dto::CapacityDefaultsUpdateRequest {
                mutation: mutation(1),
                max_active_per_harness: 3,
                max_active_per_model: 1,
                max_active_per_usage_pool: 5,
            })
            .expect_err("the stale version is rejected");
        assert_eq!(refused.code, kanban_dto::ErrorCode::StaleVersion);
    }

    #[test]
    fn a_project_without_caps_reads_unset_and_the_first_write_inserts() {
        let database = database();
        let store = SqliteCapacityStore::new(&database);
        let project = register_project(&database, "CORE");

        let unset = store.project_caps(project).expect("the caps serve");
        assert_eq!(
            unset,
            kanban_dto::CapacityProjectCaps {
                max_active_per_harness: None,
                max_active_per_model: None,
                max_active_per_usage_pool: None,
                max_active_lanes: None,
                version: 1,
            }
        );

        let imposed = store
            .update_project_caps(&kanban_dto::CapacitySettingsUpdateRequest {
                mutation: mutation(1),
                project_id: project,
                max_active_per_harness: Some(2),
                max_active_per_model: None,
                max_active_per_usage_pool: None,
                max_active_lanes: Some(3),
            })
            .expect("the first update inserts");
        assert_eq!(imposed.max_active_per_harness, Some(2));
        assert_eq!(imposed.max_active_lanes, Some(3));
        assert_eq!(imposed.version, 2);

        let stale = store
            .update_project_caps(&kanban_dto::CapacitySettingsUpdateRequest {
                mutation: mutation(1),
                project_id: project,
                max_active_per_harness: None,
                max_active_per_model: None,
                max_active_per_usage_pool: None,
                max_active_lanes: Some(1),
            })
            .expect_err("the stale version is rejected");
        assert_eq!(stale.code, kanban_dto::ErrorCode::StaleVersion);

        let cleared = store
            .update_project_caps(&kanban_dto::CapacitySettingsUpdateRequest {
                mutation: mutation(2),
                project_id: project,
                max_active_per_harness: None,
                max_active_per_model: None,
                max_active_per_usage_pool: None,
                max_active_lanes: Some(3),
            })
            .expect("the clearing update lands");
        assert_eq!(cleared.max_active_per_harness, None);
        assert_eq!(cleared.version, 3);
    }

    #[test]
    fn an_absent_row_rejects_any_version_but_the_unset_one() {
        let database = database();
        let store = SqliteCapacityStore::new(&database);
        let project = register_project(&database, "WAVE");

        let refused = store
            .update_project_caps(&kanban_dto::CapacitySettingsUpdateRequest {
                mutation: mutation(7),
                project_id: project,
                max_active_per_harness: None,
                max_active_per_model: None,
                max_active_per_usage_pool: None,
                max_active_lanes: Some(2),
            })
            .expect_err("an absent row serves only the unset version");
        assert_eq!(refused.code, kanban_dto::ErrorCode::StaleVersion);
    }
}
