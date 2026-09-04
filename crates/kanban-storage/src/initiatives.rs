//! The SQLite implementation of the Initiative storage port: rows
//! in `initiatives`, and the application's timeline envelope landing
//! unchanged in the same transaction as every change.

use kanban_app::{InitiativeStore, TimelineEnvelope};
use kanban_domain::{Initiative, InitiativeId, InitiativeName, InitiativeState};
use kanban_dto::ApiError;
use rusqlite::params;

use crate::db::{ConnectionHandle, Database, WriteSpan};
use crate::timeline::insert_event;

/// The Initiative port over the authoritative database.
pub struct SqliteInitiativeStore {
    conn: ConnectionHandle,
}

impl SqliteInitiativeStore {
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

impl InitiativeStore for SqliteInitiativeStore {
    fn create(
        &self,
        name: &InitiativeName,
        envelope: &dyn Fn(InitiativeId) -> TimelineEnvelope,
    ) -> Result<Initiative, ApiError> {
        let conn = self.lock();
        let span = WriteSpan::begin(&conn).map_err(internal)?;
        span.execute(
            "INSERT INTO initiatives (name, archived, version) VALUES (?1, 0, 1)",
            params![name.as_str()],
        )
        .map_err(internal)?;
        let id = InitiativeId::new(
            span.last_insert_rowid()
                .try_into()
                .map_err(|_| ApiError::internal("the Initiative identity overflowed"))?,
        );
        append_timeline(&span, &envelope(id))?;
        span.commit().map_err(internal)?;
        Ok(Initiative::new(id, name.clone()))
    }

    fn find(&self, id: InitiativeId) -> Result<Option<Initiative>, ApiError> {
        let conn = self.lock();
        let row = conn.query_row(
            "SELECT id, name, archived, version FROM initiatives WHERE id = ?1",
            params![id.value() as i64],
            decode_row,
        );
        match row {
            Ok(initiative) => Ok(Some(initiative)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(error) => Err(ApiError::internal(&error.to_string())),
        }
    }

    fn save(&self, initiative: &Initiative, envelope: TimelineEnvelope) -> Result<(), ApiError> {
        let conn = self.lock();
        let span = WriteSpan::begin(&conn).map_err(internal)?;
        let archived = initiative.is_archived();
        let preceding_version = initiative.version() - 1;
        let changed = span
            .execute(
                "UPDATE initiatives
                 SET name = ?2,
                     archived = ?3,
                     version = ?4,
                     archived_at = CASE
                         WHEN ?3 = 1 THEN strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
                         ELSE archived_at
                     END
                 WHERE id = ?1 AND version = ?5",
                params![
                    initiative.id().value() as i64,
                    initiative.name(),
                    archived,
                    initiative.version() as i64,
                    preceding_version as i64,
                ],
            )
            .map_err(internal)?;
        if changed != 1 {
            let current = span.query_row(
                "SELECT version FROM initiatives WHERE id = ?1",
                params![initiative.id().value() as i64],
                |row| row.get::<_, i64>(0),
            );
            return match current {
                Ok(current) => Err(ApiError::stale_version(
                    preceding_version,
                    current.unsigned_abs(),
                )),
                Err(rusqlite::Error::QueryReturnedNoRows) => Err(ApiError::not_found(&format!(
                    "initiative {}",
                    initiative.id()
                ))),
                Err(error) => Err(ApiError::internal(&error.to_string())),
            };
        }
        append_timeline(&span, &envelope)?;
        span.commit().map_err(internal)?;
        Ok(())
    }

    fn list(&self) -> Result<Vec<Initiative>, ApiError> {
        let conn = self.lock();
        let mut statement = conn
            .prepare("SELECT id, name, archived, version FROM initiatives ORDER BY id")
            .map_err(|error| ApiError::internal(&error.to_string()))?;
        let rows = statement
            .query_map([], decode_row)
            .map_err(|error| ApiError::internal(&error.to_string()))?;
        let mut initiatives = Vec::new();
        for row in rows {
            initiatives.push(row.map_err(|error| ApiError::internal(&error.to_string()))?);
        }
        Ok(initiatives)
    }
}

/// Decode one stored row: id, name, archived flag, version.
fn decode_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Initiative> {
    let id = row.get::<_, i64>(0)?.unsigned_abs();
    let name: String = row.get(1)?;
    let archived: i64 = row.get(2)?;
    let version = row.get::<_, i64>(3)?.unsigned_abs();
    let state = if archived == 1 {
        InitiativeState::Archived
    } else {
        InitiativeState::Active
    };
    // Stored names passed validation on the way in; a blank one is
    // corruption the caller must hear about, not silently accept.
    let name = InitiativeName::new(&name)
        .map_err(|_| rusqlite::Error::ToSqlConversionFailure(Box::new(CorruptName)))?;
    Ok(Initiative::restore(
        InitiativeId::new(id),
        name,
        state,
        version,
    ))
}

/// Report a SQLite failure the caller cannot act on.
fn internal(error: rusqlite::Error) -> ApiError {
    ApiError::internal(&error.to_string())
}

/// A stored name failed domain validation.
#[derive(Debug)]
struct CorruptName;

impl std::fmt::Display for CorruptName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "a stored Initiative name failed validation")
    }
}

impl std::error::Error for CorruptName {}

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
    use kanban_app::{InitiativeStore, TimelineEnvelope};
    use kanban_domain::{Initiative, InitiativeId, InitiativeName};
    use kanban_dto::{ErrorCode, TimelineEntityKind, TimelineEntityRef, TimelineEventKind};
    use serde_json::json;

    use super::SqliteInitiativeStore;
    use crate::db::Database;
    use crate::migrations::AllowAllMigrations;
    use crate::test_support::scratch_database;

    fn store() -> (tempfile::TempDir, Database, SqliteInitiativeStore) {
        let (dir, mut database) = scratch_database();
        database
            .migrate(&AllowAllMigrations)
            .expect("the migrations apply");
        let store = SqliteInitiativeStore::new(&database);
        (dir, database, store)
    }

    /// The envelope the application layer builds for one Initiative
    /// transition, as the store receives it.
    fn transition(id: InitiativeId, action: &str, facts: serde_json::Value) -> TimelineEnvelope {
        let mut detail = facts;
        let object = detail.as_object_mut().expect("the facts are an object");
        object.insert("action".to_owned(), serde_json::Value::from(action));
        object.insert("id".to_owned(), serde_json::Value::from(id.value()));
        TimelineEnvelope::global(
            TimelineEventKind::Transition,
            Some(TimelineEntityRef {
                kind: TimelineEntityKind::Initiative,
                id: id.value().to_string(),
            }),
            detail,
        )
    }

    /// The envelope builder a create hands the store.
    fn created(name: &'static str) -> impl Fn(InitiativeId) -> TimelineEnvelope {
        move |id| transition(id, "created", json!({ "name": name }))
    }

    fn stored_rows(database: &Database) -> Vec<(i64, String, i64, i64)> {
        let conn = database.connection();
        let mut statement = conn
            .prepare("SELECT id, name, archived, version FROM initiatives ORDER BY id")
            .expect("the initiatives table is readable");
        statement
            .query_map([], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })
            .expect("the row query runs")
            .collect::<Result<Vec<_>, _>>()
            .expect("the rows decode")
    }

    fn timeline_rows(database: &Database) -> Vec<(String, String, String, serde_json::Value)> {
        let conn = database.connection();
        let mut statement = conn
            .prepare("SELECT scope, project_id, kind, detail FROM timeline_events ORDER BY id")
            .expect("the timeline is readable");
        statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    serde_json::from_str(&row.get::<_, String>(3)?)
                        .expect("the stored detail is JSON"),
                ))
            })
            .expect("the timeline query runs")
            .collect::<Result<Vec<_>, _>>()
            .expect("the timeline rows decode")
    }

    #[test]
    fn creating_lands_the_row_and_its_timeline_append() {
        let (_dir, database, store) = store();

        let initiative = store
            .create(
                &InitiativeName::new("Reliability").expect("the name validates"),
                &created("Reliability"),
            )
            .expect("the create lands");

        assert_eq!(initiative.id().value(), 1);
        assert_eq!(
            stored_rows(&database),
            vec![(1, "Reliability".to_owned(), 0, 1)]
        );
        assert_eq!(
            timeline_rows(&database),
            vec![(
                "global".to_owned(),
                String::new(),
                "transition".to_owned(),
                json!({ "name": "Reliability", "action": "created", "id": 1 }),
            )],
            "the envelope reaches the row unchanged"
        );
    }

    #[test]
    fn finding_returns_the_stored_initiative_or_none() {
        let (_dir, _database, store) = store();
        store
            .create(
                &InitiativeName::new("Alpha").expect("the name validates"),
                &created("Alpha"),
            )
            .expect("the create lands");

        let found = store
            .find(InitiativeId::new(1))
            .expect("the find serves")
            .expect("the Initiative exists");
        assert_eq!(found.name(), "Alpha");
        assert_eq!(found.version(), 1);
        assert!(
            store
                .find(InitiativeId::new(9))
                .expect("the find serves")
                .is_none()
        );
    }

    #[test]
    fn saving_persists_the_transition_and_its_append() {
        let (_dir, database, store) = store();
        let mut initiative = store
            .create(
                &InitiativeName::new("Alpha").expect("the name validates"),
                &created("Alpha"),
            )
            .expect("the create lands");
        initiative
            .rename(InitiativeName::new("Beta").expect("the name validates"))
            .expect("active renames");

        store
            .save(
                &initiative,
                transition(
                    initiative.id(),
                    "renamed",
                    json!({ "from": "Alpha", "to": "Beta" }),
                ),
            )
            .expect("the save lands");

        assert_eq!(stored_rows(&database), vec![(1, "Beta".to_owned(), 0, 2)]);
        assert_eq!(
            timeline_rows(&database)
                .last()
                .cloned()
                .expect("the rename appended"),
            (
                "global".to_owned(),
                String::new(),
                "transition".to_owned(),
                json!({ "from": "Alpha", "to": "Beta", "action": "renamed", "id": 1 }),
            )
        );
    }

    #[test]
    fn archiving_records_the_terminal_state_and_its_timestamp() {
        let (_dir, database, store) = store();
        let mut initiative = store
            .create(
                &InitiativeName::new("Alpha").expect("the name validates"),
                &created("Alpha"),
            )
            .expect("the create lands");
        initiative.archive().expect("active archives");

        store
            .save(
                &initiative,
                transition(initiative.id(), "archived", json!({})),
            )
            .expect("the save lands");

        assert_eq!(stored_rows(&database), vec![(1, "Alpha".to_owned(), 1, 2)]);
        let archived_at: Option<String> = {
            let conn = database.connection();
            conn.query_row(
                "SELECT archived_at FROM initiatives WHERE id = 1",
                [],
                |row| row.get(0),
            )
            .expect("the archived timestamp is readable")
        };
        assert!(
            archived_at.is_some_and(|stamp| !stamp.is_empty()),
            "archiving records when it happened"
        );
    }

    #[test]
    fn saving_an_unknown_initiative_is_not_found() {
        let (_dir, _database, store) = store();
        let ghost = Initiative::new(
            InitiativeId::new(9),
            InitiativeName::new("Ghost").expect("the name validates"),
        );

        let error = store
            .save(&ghost, transition(ghost.id(), "renamed", json!({})))
            .expect_err("the unknown Initiative is refused");

        assert_eq!(error.code, ErrorCode::NotFound);
    }

    #[test]
    fn listing_covers_every_initiative_in_id_order() {
        let (_dir, _database, store) = store();
        for name in ["Alpha", "Beta"] {
            store
                .create(
                    &InitiativeName::new(name).expect("the name validates"),
                    &move |id| transition(id, "created", json!({ "name": name })),
                )
                .expect("the create lands");
        }
        let mut first = store
            .find(InitiativeId::new(1))
            .expect("the find serves")
            .expect("the Initiative exists");
        first.archive().expect("active archives");
        store
            .save(&first, transition(first.id(), "archived", json!({})))
            .expect("the save lands");

        let listed = store.list().expect("the list serves");

        let recorded: Vec<(String, bool, u64)> = listed
            .iter()
            .map(|initiative| {
                (
                    initiative.name().to_owned(),
                    initiative.is_archived(),
                    initiative.version(),
                )
            })
            .collect();
        assert_eq!(
            recorded,
            vec![("Alpha".to_owned(), true, 2), ("Beta".to_owned(), false, 1),],
            "archived Initiatives stay listed"
        );
    }

    #[test]
    fn deleting_an_initiative_is_refused_by_the_schema() {
        let (_dir, _database, store) = store();
        store
            .create(
                &InitiativeName::new("Alpha").expect("the name validates"),
                &created("Alpha"),
            )
            .expect("the create lands");

        let outcome = store
            .lock()
            .execute("DELETE FROM initiatives WHERE id = 1", []);

        let error = outcome.expect_err("the schema must refuse deletes");
        assert!(
            error.to_string().contains("never deleted"),
            "the refusal should say never deleted, got: {error}"
        );
        let survivor = store
            .find(InitiativeId::new(1))
            .expect("the find serves")
            .expect("the Initiative survives");
        assert_eq!(survivor.name(), "Alpha");
    }

    #[test]
    fn the_store_serves_through_a_shared_connection() {
        let (_dir, _database, store) = store();
        let boxed: Box<dyn InitiativeStore> = Box::new(store);

        // The boxed trait object must be usable from another thread:
        // the core serves commands across transport threads.
        let served = std::thread::spawn(move || {
            boxed
                .create(
                    &InitiativeName::new("Threaded").expect("the name validates"),
                    &created("Threaded"),
                )
                .map(|initiative| initiative.name().to_owned())
        })
        .join()
        .expect("the serving thread finishes");

        assert_eq!(
            served.expect("the threaded create lands"),
            "Threaded",
            "the port is Send + Sync over one connection"
        );
    }

    /// Version-guard behaviour for Initiative persistence (KAN-T76).
    mod initiative_version {
        use std::sync::Arc;

        use super::{created, store, stored_rows, timeline_rows, transition};
        use kanban_app::InitiativeStore;
        use kanban_domain::InitiativeName;
        use kanban_dto::ErrorCode;
        use serde_json::json;

        #[test]
        fn saving_updates_by_identity_and_the_preceding_version() {
            let (_dir, database, store) = store();
            let mut initiative = store
                .create(
                    &InitiativeName::new("Alpha").expect("the name validates"),
                    &created("Alpha"),
                )
                .expect("the create lands");
            initiative
                .rename(InitiativeName::new("Beta").expect("the name validates"))
                .expect("active renames");

            store
                .save(
                    &initiative,
                    transition(
                        initiative.id(),
                        "renamed",
                        json!({ "from": "Alpha", "to": "Beta" }),
                    ),
                )
                .expect("the guarded save lands");

            assert_eq!(
                stored_rows(&database),
                vec![(1, "Beta".to_owned(), 0, 2)],
                "the row must carry the new facts at version two"
            );
        }

        #[test]
        fn a_stale_storage_write_returns_stale_version_without_a_timeline_row() {
            let (_dir, database, store) = store();
            let initiative = store
                .create(
                    &InitiativeName::new("Alpha").expect("the name validates"),
                    &created("Alpha"),
                )
                .expect("the create lands");
            let mut stale = initiative.clone();
            stale
                .rename(InitiativeName::new("Beta").expect("the name validates"))
                .expect("active renames");
            let mut current = initiative;
            current
                .rename(InitiativeName::new("Gamma").expect("the name validates"))
                .expect("active renames");
            store
                .save(
                    &current,
                    transition(
                        current.id(),
                        "renamed",
                        json!({ "from": "Alpha", "to": "Gamma" }),
                    ),
                )
                .expect("the first save lands");

            let timeline_before = timeline_rows(&database).len();

            let error = store
                .save(
                    &stale,
                    transition(
                        stale.id(),
                        "renamed",
                        json!({ "from": "Alpha", "to": "Beta" }),
                    ),
                )
                .expect_err("the stale save is refused");

            assert_eq!(error.code, ErrorCode::StaleVersion);
            assert_eq!(error.current_version, Some(2));
            assert_eq!(
                timeline_rows(&database).len(),
                timeline_before,
                "a stale save must not append a timeline row"
            );
            assert_eq!(
                stored_rows(&database),
                vec![(1, "Gamma".to_owned(), 0, 2)],
                "the winning write must remain authoritative"
            );
        }

        #[test]
        fn racing_two_copies_commits_exactly_one_transition() {
            let (_dir, database, store) = store();
            let store = Arc::new(store);
            let initiative = store
                .create(
                    &InitiativeName::new("Alpha").expect("the name validates"),
                    &created("Alpha"),
                )
                .expect("the create lands");

            let mut copy_a = initiative.clone();
            let mut copy_b = initiative;
            copy_a
                .rename(InitiativeName::new("Alpha-wins").expect("the name validates"))
                .expect("active renames");
            copy_b
                .rename(InitiativeName::new("Beta-wins").expect("the name validates"))
                .expect("active renames");

            let store_a = store.clone();
            let store_b = store.clone();
            let handle_a = std::thread::spawn(move || {
                let id = copy_a.id();
                store_a.save(
                    &copy_a,
                    transition(
                        id,
                        "renamed",
                        json!({ "from": "Alpha", "to": "Alpha-wins" }),
                    ),
                )
            });
            let handle_b = std::thread::spawn(move || {
                let id = copy_b.id();
                store_b.save(
                    &copy_b,
                    transition(id, "renamed", json!({ "from": "Alpha", "to": "Beta-wins" })),
                )
            });

            let result_a = handle_a.join().expect("the first thread finishes");
            let result_b = handle_b.join().expect("the second thread finishes");
            let outcomes = [result_a, result_b];
            let successes = outcomes.iter().filter(|result| result.is_ok()).count();
            let failures: Vec<_> = outcomes
                .into_iter()
                .filter_map(|result| result.err())
                .collect();

            assert_eq!(successes, 1, "exactly one transition may commit");
            assert_eq!(failures.len(), 1, "the loser must be refused");
            assert_eq!(failures[0].code, ErrorCode::StaleVersion);
            assert_eq!(
                timeline_rows(&database).len(),
                2,
                "only the create and one rename may append"
            );
            let (_, name, _, version) = stored_rows(&database)
                .into_iter()
                .next()
                .expect("the Initiative survives");
            assert_eq!(version, 2);
            assert!(
                name == "Alpha-wins" || name == "Beta-wins",
                "the committed name must come from the winning save, got {name}"
            );
        }
    }
}
