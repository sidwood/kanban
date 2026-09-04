//! The SQLite implementation of the Initiative storage port: rows
//! in `initiatives`, and the timeline append landing in the same
//! transaction as every change.

use kanban_app::{InitiativeStore, TimelineAppend};
use kanban_domain::{Initiative, InitiativeId, InitiativeName, InitiativeState};
use kanban_dto::ApiError;
use rusqlite::params;
use serde_json::Value;

use crate::db::{ConnectionHandle, Database};

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
    fn lock(&self) -> std::sync::MutexGuard<'_, rusqlite::Connection> {
        self.conn
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

impl InitiativeStore for SqliteInitiativeStore {
    fn create(
        &self,
        name: &InitiativeName,
        append: TimelineAppend,
    ) -> Result<Initiative, ApiError> {
        let mut conn = self.lock();
        let transaction = conn
            .transaction()
            .map_err(|error| ApiError::internal(&error.to_string()))?;
        transaction
            .execute(
                "INSERT INTO initiatives (name, archived, version) VALUES (?1, 0, 1)",
                params![name.as_str()],
            )
            .map_err(|error| ApiError::internal(&error.to_string()))?;
        let id = InitiativeId::new(
            transaction
                .last_insert_rowid()
                .try_into()
                .map_err(|_| ApiError::internal("the Initiative identity overflowed"))?,
        );
        append_timeline(&transaction, id, &append)?;
        transaction
            .commit()
            .map_err(|error| ApiError::internal(&error.to_string()))?;
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

    fn save(&self, initiative: &Initiative, append: TimelineAppend) -> Result<(), ApiError> {
        let mut conn = self.lock();
        let transaction = conn
            .transaction()
            .map_err(|error| ApiError::internal(&error.to_string()))?;
        let archived = initiative.is_archived();
        let changed = transaction
            .execute(
                "UPDATE initiatives
                 SET name = ?2,
                     archived = ?3,
                     version = ?4,
                     archived_at = CASE
                         WHEN ?3 = 1 THEN strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
                         ELSE archived_at
                     END
                 WHERE id = ?1",
                params![
                    initiative.id().value() as i64,
                    initiative.name(),
                    archived,
                    initiative.version() as i64
                ],
            )
            .map_err(|error| ApiError::internal(&error.to_string()))?;
        if changed != 1 {
            return Err(ApiError::not_found(&format!(
                "initiative {}",
                initiative.id()
            )));
        }
        append_timeline(&transaction, initiative.id(), &append)?;
        transaction
            .commit()
            .map_err(|error| ApiError::internal(&error.to_string()))?;
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

/// A stored name failed domain validation.
#[derive(Debug)]
struct CorruptName;

impl std::fmt::Display for CorruptName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "a stored Initiative name failed validation")
    }
}

impl std::error::Error for CorruptName {}

/// Append the change's timeline entry, with the Initiative's
/// identity in the facts, on the same transaction as the row.
fn append_timeline(
    transaction: &rusqlite::Transaction<'_>,
    id: InitiativeId,
    append: &TimelineAppend,
) -> Result<(), ApiError> {
    let mut detail = append.facts.clone();
    detail
        .as_object_mut()
        .ok_or_else(|| ApiError::internal("timeline facts must be a JSON object"))?
        .insert("id".to_owned(), Value::from(id.value()));
    crate::timeline::insert_event(transaction, append.kind, &detail)
        .map_err(|error| ApiError::internal(&error.to_string()))
}

#[cfg(test)]
mod tests {
    use kanban_app::{InitiativeStore, TimelineAppend};
    use kanban_domain::{Initiative, InitiativeId, InitiativeName};
    use kanban_dto::ErrorCode;
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

    fn append(kind: &'static str, facts: serde_json::Value) -> TimelineAppend {
        TimelineAppend { kind, facts }
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

    fn timeline_rows(database: &Database) -> Vec<(String, serde_json::Value)> {
        let conn = database.connection();
        let mut statement = conn
            .prepare("SELECT kind, detail FROM timeline_events ORDER BY id")
            .expect("the timeline is readable");
        statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    serde_json::from_str(&row.get::<_, String>(1)?)
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
                append("initiative.created", json!({ "name": "Reliability" })),
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
                "initiative.created".to_owned(),
                json!({ "name": "Reliability", "id": 1 })
            )]
        );
    }

    #[test]
    fn finding_returns_the_stored_initiative_or_none() {
        let (_dir, _database, store) = store();
        store
            .create(
                &InitiativeName::new("Alpha").expect("the name validates"),
                append("initiative.created", json!({ "name": "Alpha" })),
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
                append("initiative.created", json!({ "name": "Alpha" })),
            )
            .expect("the create lands");
        initiative
            .rename(InitiativeName::new("Beta").expect("the name validates"))
            .expect("active renames");

        store
            .save(
                &initiative,
                append(
                    "initiative.renamed",
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
                "initiative.renamed".to_owned(),
                json!({ "from": "Alpha", "to": "Beta", "id": 1 })
            )
        );
    }

    #[test]
    fn archiving_records_the_terminal_state_and_its_timestamp() {
        let (_dir, database, store) = store();
        let mut initiative = store
            .create(
                &InitiativeName::new("Alpha").expect("the name validates"),
                append("initiative.created", json!({ "name": "Alpha" })),
            )
            .expect("the create lands");
        initiative.archive().expect("active archives");

        store
            .save(&initiative, append("initiative.archived", json!({})))
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
            .save(&ghost, append("initiative.renamed", json!({})))
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
                    append("initiative.created", json!({ "name": name })),
                )
                .expect("the create lands");
        }
        let mut first = store
            .find(InitiativeId::new(1))
            .expect("the find serves")
            .expect("the Initiative exists");
        first.archive().expect("active archives");
        store
            .save(&first, append("initiative.archived", json!({})))
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
                append("initiative.created", json!({ "name": "Alpha" })),
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
                    append("initiative.created", json!({ "name": "Threaded" })),
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
}
