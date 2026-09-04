//! The SQLite record of spent idempotency keys: the outcome that
//! replays a retried mutation, committed in the same transaction as
//! the mutation itself (DR-SS-03, KAN-S1-US2).

use std::num::NonZeroU32;

use kanban_app::{IdempotencyStore, MutationSpan, RecordedOutcome};
use kanban_dto::ApiError;
use parking_lot::ReentrantMutexGuard;
use rusqlite::{Connection, params};

use crate::db::{self, ConnectionHandle, Database};

/// How many replay outcomes the database keeps.
///
/// A retry follows its original within seconds, so the store holds
/// far more outcomes than any client could still be retrying and
/// prunes the rest in the commit that records the newest: replay
/// stays available and the table cannot grow without end.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetentionPolicy {
    retained: NonZeroU32,
}

impl RetentionPolicy {
    /// Keep the `retained` most recently recorded outcomes. The
    /// bound is never zero: a store that pruned the outcome it just
    /// recorded could not replay anything.
    pub const fn keep_most_recent(retained: NonZeroU32) -> Self {
        Self { retained }
    }

    /// The number of outcomes kept.
    pub const fn retained(self) -> NonZeroU32 {
        self.retained
    }
}

/// The idempotency port over the authoritative database.
pub struct SqliteIdempotencyStore {
    conn: ConnectionHandle,
    retention: RetentionPolicy,
}

impl SqliteIdempotencyStore {
    /// Share the connection the `database` owns, keeping outcomes
    /// under `retention`.
    pub fn new(database: &Database, retention: RetentionPolicy) -> Self {
        Self {
            conn: database.connection_handle(),
            retention,
        }
    }
}

impl IdempotencyStore for SqliteIdempotencyStore {
    fn recorded(&self, key: &str) -> Result<Option<RecordedOutcome>, ApiError> {
        let conn = self.conn.lock();
        let row = conn.query_row(
            "SELECT fingerprint, response FROM idempotency_outcomes
             WHERE idempotency_key = ?1",
            params![key],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        );
        match row {
            Ok((fingerprint, response)) => Ok(Some(RecordedOutcome {
                fingerprint,
                response: serde_json::from_str(&response)
                    .map_err(|error| ApiError::internal(&error.to_string()))?,
            })),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(error) => Err(internal(error)),
        }
    }

    fn begin(&self) -> Result<Box<dyn MutationSpan + '_>, ApiError> {
        // Holding the connection is what makes the span atomic: every
        // store the handler writes through shares this connection, so
        // its rows join this transaction and no other thread can read
        // them until the outcome commits with them.
        let conn = self.conn.lock();
        db::open_span(&conn).map_err(internal)?;
        Ok(Box::new(SqliteMutationSpan {
            conn,
            retention: self.retention,
            committed: false,
        }))
    }
}

/// One mutation's durable span: the connection it holds, and the
/// bound applied when its outcome lands.
struct SqliteMutationSpan<'a> {
    conn: ReentrantMutexGuard<'a, Connection>,
    retention: RetentionPolicy,
    committed: bool,
}

impl MutationSpan for SqliteMutationSpan<'_> {
    fn commit(mut self: Box<Self>, key: &str, outcome: RecordedOutcome) -> Result<(), ApiError> {
        let response = serde_json::to_string(&outcome.response)
            .map_err(|error| ApiError::internal(&error.to_string()))?;
        self.conn
            .execute(
                "INSERT INTO idempotency_outcomes (idempotency_key, fingerprint, response)
                 VALUES (?1, ?2, ?3)",
                params![key, outcome.fingerprint, response],
            )
            .map_err(internal)?;
        prune(&self.conn, self.retention)?;
        db::land_span(&self.conn).map_err(internal)?;
        self.committed = true;
        Ok(())
    }
}

impl Drop for SqliteMutationSpan<'_> {
    fn drop(&mut self) {
        if !self.committed {
            db::discard_span(&self.conn);
        }
    }
}

/// Drop every outcome older than the retained bound. It runs inside
/// the recording span, so the bound holds at every commit.
fn prune(conn: &Connection, retention: RetentionPolicy) -> Result<(), ApiError> {
    conn.execute(
        "DELETE FROM idempotency_outcomes
          WHERE id NOT IN (
              SELECT id FROM idempotency_outcomes ORDER BY id DESC LIMIT ?1
          )",
        params![i64::from(retention.retained().get())],
    )
    .map_err(internal)?;
    Ok(())
}

/// Report a SQLite failure the caller cannot act on.
fn internal(error: rusqlite::Error) -> ApiError {
    ApiError::internal(&error.to_string())
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU32;

    use kanban_app::{IdempotencyStore, InitiativeStore, RecordedOutcome, TimelineAppend};
    use kanban_domain::InitiativeName;
    use serde_json::{Value, json};
    use tempfile::TempDir;

    use super::{RetentionPolicy, SqliteIdempotencyStore};
    use crate::db::Database;
    use crate::initiatives::SqliteInitiativeStore;
    use crate::migrations::AllowAllMigrations;
    use crate::test_support::scratch_database;

    /// A migrated scratch database.
    fn migrated() -> (TempDir, Database) {
        let (dir, mut database) = scratch_database();
        database
            .migrate(&AllowAllMigrations)
            .expect("the migrations apply");
        (dir, database)
    }

    /// Keep the `retained` most recent outcomes.
    fn keep(retained: u32) -> RetentionPolicy {
        RetentionPolicy::keep_most_recent(NonZeroU32::new(retained).expect("the bound is not zero"))
    }

    /// One recorded outcome, named by `label`.
    fn outcome(label: &str) -> RecordedOutcome {
        RecordedOutcome {
            fingerprint: format!("initiative:{{\"name\":\"{label}\"}}"),
            response: json!({ "id": 1, "name": label, "archived": false, "version": 1 }),
        }
    }

    /// Every stored Initiative name, in insertion order.
    fn initiative_names(database: &Database) -> Vec<String> {
        let conn = database.connection();
        let mut statement = conn
            .prepare("SELECT name FROM initiatives ORDER BY id")
            .expect("the initiatives table is readable");
        statement
            .query_map([], |row| row.get(0))
            .expect("the query runs")
            .collect::<Result<Vec<_>, _>>()
            .expect("the names decode")
    }

    /// Write one Initiative through the real storage port.
    fn create_initiative(store: &SqliteInitiativeStore, name: &str) {
        store
            .create(
                &InitiativeName::new(name).expect("the name validates"),
                TimelineAppend {
                    kind: "initiative.created",
                    facts: json!({ "name": name }),
                },
            )
            .expect("the create writes");
    }

    #[test]
    fn an_unspent_key_has_no_outcome() {
        let (_dir, database) = migrated();
        let store = SqliteIdempotencyStore::new(&database, keep(8));

        assert!(
            store
                .recorded("key-1")
                .expect("the lookup serves")
                .is_none()
        );
    }

    #[test]
    fn a_commit_records_the_key_fingerprint_response_and_time() {
        let (_dir, database) = migrated();
        let store = SqliteIdempotencyStore::new(&database, keep(8));

        store
            .begin()
            .expect("the span opens")
            .commit("key-1", outcome("Reliability"))
            .expect("the span commits");

        let conn = database.connection();
        let (fingerprint, response, recorded_at): (String, String, String) = conn
            .query_row(
                "SELECT fingerprint, response, recorded_at
                 FROM idempotency_outcomes WHERE idempotency_key = ?1",
                ["key-1"],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("the recorded row is readable");
        assert_eq!(fingerprint, "initiative:{\"name\":\"Reliability\"}");
        assert_eq!(
            serde_json::from_str::<Value>(&response).expect("the response is JSON"),
            outcome("Reliability").response
        );
        assert!(
            recorded_at.ends_with('Z'),
            "the outcome records when it landed: {recorded_at}"
        );
    }

    #[test]
    fn a_mutation_and_its_outcome_land_in_one_commit() {
        let (_dir, database) = migrated();
        let initiatives = SqliteInitiativeStore::new(&database);
        let store = SqliteIdempotencyStore::new(&database, keep(8));

        let span = store.begin().expect("the span opens");
        create_initiative(&initiatives, "Reliability");
        span.commit("key-1", outcome("Reliability"))
            .expect("the span commits");

        assert_eq!(initiative_names(&database), vec!["Reliability".to_owned()]);
        assert_eq!(
            store.recorded("key-1").expect("the lookup serves"),
            Some(outcome("Reliability"))
        );
    }

    #[test]
    fn a_span_dropped_without_committing_discards_the_mutation_and_its_outcome() {
        let (_dir, database) = migrated();
        let initiatives = SqliteInitiativeStore::new(&database);
        let store = SqliteIdempotencyStore::new(&database, keep(8));

        let span = store.begin().expect("the span opens");
        create_initiative(&initiatives, "Reliability");
        drop(span);

        assert!(
            initiative_names(&database).is_empty(),
            "a mutation cannot survive the span that would have recorded it"
        );
        assert!(
            store
                .recorded("key-1")
                .expect("the lookup serves")
                .is_none()
        );
    }

    #[test]
    fn a_recorded_outcome_survives_reopening_the_database() {
        let dir = tempfile::tempdir().expect("a scratch directory is available");
        let path = dir.path().join("kanban.sqlite");
        {
            let mut database = Database::open(&path).expect("the first open succeeds");
            database
                .migrate(&AllowAllMigrations)
                .expect("the migrations apply");
            SqliteIdempotencyStore::new(&database, keep(8))
                .begin()
                .expect("the span opens")
                .commit("key-1", outcome("Reliability"))
                .expect("the span commits");
        }

        let mut database = Database::open(&path).expect("the second open succeeds");
        database
            .migrate(&AllowAllMigrations)
            .expect("the second run applies nothing");
        let store = SqliteIdempotencyStore::new(&database, keep(8));

        assert_eq!(
            store.recorded("key-1").expect("the lookup serves"),
            Some(outcome("Reliability")),
            "a replay outcome outlives the process that recorded it"
        );
    }

    #[test]
    fn retention_keeps_only_the_most_recent_outcomes() {
        let (_dir, database) = migrated();
        let store = SqliteIdempotencyStore::new(&database, keep(2));

        for label in ["First", "Second", "Third", "Fourth"] {
            store
                .begin()
                .expect("the span opens")
                .commit(label, outcome(label))
                .expect("the span commits");
        }

        let spent: Vec<&str> = ["First", "Second", "Third", "Fourth"]
            .into_iter()
            .filter(|key| store.recorded(key).expect("the lookup serves").is_some())
            .collect();
        assert_eq!(
            spent,
            vec!["Third", "Fourth"],
            "the bound drops the oldest outcomes and keeps the newest"
        );
    }

    #[test]
    fn a_recorded_outcome_cannot_be_rewritten() {
        let (_dir, database) = migrated();
        let store = SqliteIdempotencyStore::new(&database, keep(8));
        store
            .begin()
            .expect("the span opens")
            .commit("key-1", outcome("Reliability"))
            .expect("the span commits");

        let error = database
            .connection()
            .execute(
                "UPDATE idempotency_outcomes SET response = '{}' WHERE idempotency_key = 'key-1'",
                [],
            )
            .expect_err("a spent key's outcome is fixed");

        assert!(
            error.to_string().contains("write-once"),
            "the refusal should say write-once, got: {error}"
        );
    }
}
