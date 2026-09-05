//! The SQLite implementation of the Execution Profile storage port:
//! rows in `execution_profiles`, and the application's timeline
//! envelope landing unchanged in the same transaction as every
//! change. Names are unique at the schema level and nothing is ever
//! deleted; every stored value passed domain validation on the way
//! in, so a row that fails to rehydrate is corruption the caller
//! must hear about.

use kanban_app::{ProfileStore, TimelineEnvelope};
use kanban_domain::{ExecutionProfile, ProfileDefinition, ProfileName, ProfileState};
use kanban_dto::ApiError;
use rusqlite::params;

use crate::db::{ConnectionHandle, Database, WriteSpan};
use crate::timeline::insert_event;

/// Every stored column of one profile row, in select order.
const PROFILE_COLUMNS: &str =
    "name, harness, model, effort, usage_pool, fallback, retired, version";

/// The profile port over the authoritative database.
pub struct SqliteProfileStore {
    conn: ConnectionHandle,
}

impl SqliteProfileStore {
    /// Share the connection the `database` owns.
    pub fn new(database: &Database) -> Self {
        Self {
            conn: database.connection_handle(),
        }
    }

    /// Lock the shared connection.
    fn lock(&self) -> parking_lot::ReentrantMutexGuard<'_, rusqlite::Connection> {
        self.conn.lock()
    }
}

impl ProfileStore for SqliteProfileStore {
    fn define(
        &self,
        profile: &ExecutionProfile,
        envelope: &TimelineEnvelope,
    ) -> Result<(), ApiError> {
        let conn = self.lock();
        let span = WriteSpan::begin(&conn).map_err(internal)?;
        let outcome = span.execute(
            "INSERT INTO execution_profiles
                 (name, harness, model, effort, usage_pool, fallback, retired, version)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0, ?7)",
            params![
                profile.name().as_str(),
                profile.harness(),
                profile.model(),
                profile.effort(),
                profile.usage_pool(),
                profile.fallback().map(|name| name.as_str()),
                profile.version() as i64,
            ],
        );
        match outcome {
            Ok(_) => {}
            Err(rusqlite::Error::SqliteFailure(failure, _))
                if failure.code == rusqlite::ErrorCode::ConstraintViolation =>
            {
                return Err(kanban_app::duplicate_profile_name_error(
                    profile.name().as_str(),
                ));
            }
            Err(error) => return Err(internal(error)),
        }
        append_timeline(&span, envelope)?;
        span.commit().map_err(internal)?;
        Ok(())
    }

    fn save(
        &self,
        profile: &ExecutionProfile,
        envelope: &TimelineEnvelope,
    ) -> Result<(), ApiError> {
        let conn = self.lock();
        let span = WriteSpan::begin(&conn).map_err(internal)?;
        let retired = profile.is_retired();
        let preceding_version = profile.version() - 1;
        let changed = span
            .execute(
                "UPDATE execution_profiles
                 SET harness = ?2,
                     model = ?3,
                     effort = ?4,
                     usage_pool = ?5,
                     fallback = ?6,
                     retired = ?7,
                     version = ?8,
                     retired_at = CASE
                         WHEN ?7 = 1 THEN strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
                         ELSE retired_at
                     END
                 WHERE name = ?1 AND version = ?9",
                params![
                    profile.name().as_str(),
                    profile.harness(),
                    profile.model(),
                    profile.effort(),
                    profile.usage_pool(),
                    profile.fallback().map(|name| name.as_str()),
                    retired,
                    profile.version() as i64,
                    preceding_version as i64,
                ],
            )
            .map_err(internal)?;
        if changed != 1 {
            return Err(save_refused(&span, profile.name(), preceding_version));
        }
        append_timeline(&span, envelope)?;
        span.commit().map_err(internal)?;
        Ok(())
    }

    fn find(&self, name: &ProfileName) -> Result<Option<ExecutionProfile>, ApiError> {
        let conn = self.lock();
        let row = conn.query_row(
            &format!("SELECT {PROFILE_COLUMNS} FROM execution_profiles WHERE name = ?1"),
            params![name.as_str()],
            decode_row,
        );
        match row {
            Ok(profile) => Ok(Some(profile)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(error) => Err(internal(error)),
        }
    }

    fn list(&self) -> Result<Vec<ExecutionProfile>, ApiError> {
        let conn = self.lock();
        let mut statement = conn
            .prepare(&format!(
                "SELECT {PROFILE_COLUMNS} FROM execution_profiles ORDER BY id"
            ))
            .map_err(internal)?;
        let rows = statement.query_map([], decode_row).map_err(internal)?;
        let mut profiles = Vec::new();
        for row in rows {
            profiles.push(row.map_err(internal)?);
        }
        Ok(profiles)
    }
}

/// Decode one stored row into the domain aggregate. Every stored
/// value passed validation on the way in, so a failure here is
/// corruption the caller must hear about, not silently accept.
fn decode_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ExecutionProfile> {
    let name: String = row.get(0)?;
    let harness: String = row.get(1)?;
    let model: String = row.get(2)?;
    let effort: String = row.get(3)?;
    let usage_pool: String = row.get(4)?;
    let fallback: Option<String> = row.get(5)?;
    let retired: i64 = row.get(6)?;
    let version = row.get::<_, i64>(7)?.unsigned_abs();
    let corrupt = |field: &'static str| {
        rusqlite::Error::FromSqlConversionFailure(
            0,
            rusqlite::types::Type::Text,
            Box::new(CorruptRow(field)),
        )
    };
    let name = ProfileName::new(&name).map_err(|_| corrupt("name"))?;
    let fallback = fallback
        .map(|raw| ProfileName::new(&raw).map_err(|_| corrupt("fallback")))
        .transpose()?;
    let definition = ProfileDefinition::new(harness, model, effort, usage_pool, fallback)
        .map_err(|_| corrupt("definition"))?;
    let state = if retired == 1 {
        ProfileState::Retired
    } else {
        ProfileState::Active
    };
    Ok(ExecutionProfile::restore(name, definition, state, version))
}

/// Why a guarded write was refused, read from the row's current
/// state.
fn save_refused(conn: &rusqlite::Connection, name: &ProfileName, attempted_from: u64) -> ApiError {
    match conn.query_row(
        "SELECT version FROM execution_profiles WHERE name = ?1",
        params![name.as_str()],
        |row| row.get::<_, i64>(0),
    ) {
        Ok(current) => ApiError::stale_version(attempted_from, current.unsigned_abs()),
        Err(rusqlite::Error::QueryReturnedNoRows) => {
            ApiError::not_found(&format!("profile {name}"))
        }
        Err(error) => internal(error),
    }
}

/// Report a SQLite failure the caller cannot act on.
fn internal(error: rusqlite::Error) -> ApiError {
    ApiError::internal(&error.to_string())
}

/// A stored profile row failed domain validation.
#[derive(Debug)]
struct CorruptRow(&'static str);

impl std::fmt::Display for CorruptRow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "a stored execution profile row failed validation: {}",
            self.0
        )
    }
}

impl std::error::Error for CorruptRow {}

/// Insert the application's envelope, unchanged, on the same
/// transaction as the row it records.
fn append_timeline(
    conn: &rusqlite::Connection,
    envelope: &TimelineEnvelope,
) -> Result<(), ApiError> {
    insert_event(conn, envelope).map_err(|error| ApiError::internal(&error.to_string()))
}

#[cfg(test)]
mod tests {
    use kanban_app::ProfileStore;
    use kanban_domain::{ExecutionProfile, ProfileDefinition, ProfileName, ProfileState};
    use kanban_dto::{ErrorCode, TimelineEntityKind, TimelineEntityRef, TimelineEventKind};
    use serde_json::json;

    use super::SqliteProfileStore;
    use crate::db::Database;
    use crate::migrations::AllowAllMigrations;
    use crate::test_support::scratch_database;

    fn store() -> (tempfile::TempDir, Database, SqliteProfileStore) {
        let (dir, mut database) = scratch_database();
        database
            .migrate(&AllowAllMigrations)
            .expect("the migrations apply");
        let store = SqliteProfileStore::new(&database);
        (dir, database, store)
    }

    fn named(raw: &str) -> ProfileName {
        ProfileName::new(raw).expect("a non-blank name is accepted")
    }

    fn definition(fallback: Option<&str>) -> ProfileDefinition {
        ProfileDefinition::new(
            "claude-code",
            "opus",
            "high",
            "operator",
            fallback.map(named),
        )
        .expect("a complete definition is accepted")
    }

    /// The envelope the application layer builds for one catalogue
    /// change, as the store receives it.
    fn transition(name: &ProfileName, action: &str) -> kanban_app::TimelineEnvelope {
        kanban_app::TimelineEnvelope::global(
            TimelineEventKind::Transition,
            Some(TimelineEntityRef {
                kind: TimelineEntityKind::Profile,
                id: name.as_str().to_owned(),
            }),
            json!({ "action": action, "name": name.as_str() }),
        )
    }

    fn entry(name: &str, fallback: Option<&str>) -> ExecutionProfile {
        ExecutionProfile::define(named(name), definition(fallback))
            .expect("the fixture entry validates")
    }

    /// Every global profile-scoped timeline row's detail, in landing
    /// order.
    fn profile_timeline(database: &Database) -> Vec<serde_json::Value> {
        let conn = database.connection();
        let mut statement = conn
            .prepare(
                "SELECT detail FROM timeline_events
                 WHERE scope = 'global' AND entity_kind = 'profile' ORDER BY id",
            )
            .expect("the timeline is readable");
        statement
            .query_map([], |row| {
                let detail: String = row.get(0)?;
                Ok(serde_json::from_str(&detail).expect("stored detail is JSON"))
            })
            .expect("the query runs")
            .collect::<Result<Vec<_>, _>>()
            .expect("the rows decode")
    }

    #[test]
    fn defining_lands_the_row_and_the_timeline_append() {
        let (_dir, database, store) = store();
        let primary = entry("standard", None);

        store
            .define(&primary, &transition(primary.name(), "defined"))
            .expect("the entry lands");

        let found = store
            .find(&named("standard"))
            .expect("the find serves")
            .expect("the entry exists");
        assert_eq!(found, primary);
        assert_eq!(
            profile_timeline(&database),
            vec![json!({ "action": "defined", "name": "standard" })],
            "the envelope reaches the global timeline unchanged"
        );
    }

    #[test]
    fn defining_a_duplicate_name_is_refused_without_a_timeline_row() {
        let (_dir, database, store) = store();
        let primary = entry("standard", None);
        store
            .define(&primary, &transition(primary.name(), "defined"))
            .expect("the first entry lands");

        let error = store
            .define(&primary, &transition(primary.name(), "defined"))
            .expect_err("the schema keeps names unique");

        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert_eq!(
            error.message,
            "the profile name `standard` is already defined"
        );
        assert_eq!(
            profile_timeline(&database).len(),
            1,
            "the refused define appends nothing"
        );
    }

    #[test]
    fn saving_replaces_the_definition_and_guards_the_version() {
        let (_dir, _database, store) = store();
        let mut primary = entry("standard", None);
        store
            .define(&primary, &transition(primary.name(), "defined"))
            .expect("the entry lands");

        primary
            .redefine(definition(None))
            .expect("the redefine applies");
        store
            .save(&primary, &transition(primary.name(), "updated"))
            .expect("the save lands");

        let error = store
            .save(&primary, &transition(primary.name(), "updated"))
            .expect_err("the spent version is refused");
        assert_eq!(error.code, ErrorCode::StaleVersion);

        let found = store
            .find(&named("standard"))
            .expect("the find serves")
            .expect("the entry exists");
        assert_eq!(found.version(), 2);
        assert_eq!(found, primary, "the stored row round trips");
    }

    #[test]
    fn retiring_round_trips_and_the_row_is_never_deleted() {
        let (_dir, database, store) = store();
        let mut primary = entry("standard", None);
        store
            .define(&primary, &transition(primary.name(), "defined"))
            .expect("the entry lands");
        primary.retire().expect("the retire applies");
        store
            .save(&primary, &transition(primary.name(), "retired"))
            .expect("the retire lands");

        let found = store
            .find(&named("standard"))
            .expect("the find serves")
            .expect("the entry exists");
        assert_eq!(found.state(), ProfileState::Retired);
        assert_eq!(found.version(), 2);

        let outcome = database
            .connection()
            .execute("DELETE FROM execution_profiles WHERE name = 'standard'", []);
        let error = outcome.expect_err("the schema must refuse deletes");
        assert!(
            error.to_string().contains("never deleted"),
            "the refusal should say never deleted, got: {error}"
        );
    }

    #[test]
    fn a_fallback_reference_round_trips() {
        let (_dir, _database, store) = store();
        let primary = entry("standard", None);
        store
            .define(&primary, &transition(primary.name(), "defined"))
            .expect("the primary lands");
        let secondary = entry("nightly", Some("standard"));
        store
            .define(&secondary, &transition(secondary.name(), "defined"))
            .expect("the secondary lands");

        let found = store
            .find(&named("nightly"))
            .expect("the find serves")
            .expect("the entry exists");
        assert_eq!(found.fallback().map(|name| name.as_str()), Some("standard"));
        assert_eq!(
            store.list().expect("the list serves").len(),
            2,
            "listing covers every entry in definition order"
        );
    }
}
