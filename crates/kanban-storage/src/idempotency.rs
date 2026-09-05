//! The SQLite record of spent idempotency keys: the outcome that
//! replays a retried mutation, committed in the same transaction as
//! the mutation itself (DR-SS-03, KAN-S1-US2).

use std::num::NonZeroU32;
use std::time::Duration;

use kanban_app::{IdempotencyStore, MutationSpan, RecordedOutcome};
use kanban_dto::ApiError;
use parking_lot::ReentrantMutexGuard;
use rusqlite::{Connection, params};

use crate::db::{self, ConnectionHandle, Database};

/// How many replay outcomes the database keeps and how long each one
/// survives a count-bound burst.
///
/// A retry follows its original within seconds, but a client may
/// still be entitled to replay across restarts and command bursts.
/// The count bound keeps the table bounded; the minimum age keeps
/// every outcome inside the retry window even when the burst exceeds
/// the count.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetentionPolicy {
    retained: NonZeroU32,
    minimum_age: Duration,
}

impl RetentionPolicy {
    /// Keep the `retained` most recently recorded outcomes and every
    /// outcome younger than `minimum_age`. The count bound is never
    /// zero: a store that pruned the outcome it just recorded could
    /// not replay anything.
    pub const fn new(retained: NonZeroU32, minimum_age: Duration) -> Self {
        Self {
            retained,
            minimum_age,
        }
    }

    /// Keep the `retained` most recently recorded outcomes with no
    /// minimum age. Tests use this when the time floor is not the
    /// behaviour under proof.
    pub const fn keep_most_recent(retained: NonZeroU32) -> Self {
        Self::new(retained, Duration::ZERO)
    }

    /// The number of outcomes kept by count alone.
    pub const fn retained(self) -> NonZeroU32 {
        self.retained
    }

    /// The minimum age every outcome survives, even above the count
    /// bound.
    pub const fn minimum_age(self) -> Duration {
        self.minimum_age
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

/// Drop outcomes beyond both the count bound and the minimum age.
/// It runs inside the recording span, so the policy holds at every
/// commit.
fn prune(conn: &Connection, retention: RetentionPolicy) -> Result<(), ApiError> {
    if retention.minimum_age().is_zero() {
        conn.execute(
            "DELETE FROM idempotency_outcomes
              WHERE id NOT IN (
                  SELECT id FROM idempotency_outcomes ORDER BY id DESC LIMIT ?1
              )",
            params![i64::from(retention.retained().get())],
        )
        .map_err(internal)?;
    } else {
        let age_modifier = format!("-{} seconds", retention.minimum_age().as_secs());
        conn.execute(
            "DELETE FROM idempotency_outcomes
              WHERE id NOT IN (
                  SELECT id FROM idempotency_outcomes ORDER BY id DESC LIMIT ?1
              )
              AND recorded_at < strftime('%Y-%m-%dT%H:%M:%fZ', 'now', ?2)",
            params![i64::from(retention.retained().get()), age_modifier],
        )
        .map_err(internal)?;
    }
    Ok(())
}

/// Report a SQLite failure the caller cannot act on.
fn internal(error: rusqlite::Error) -> ApiError {
    ApiError::internal(&error.to_string())
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU32;
    use std::time::Duration;

    use kanban_app::{IdempotencyStore, InitiativeStore, RecordedOutcome, TimelineEnvelope};
    use kanban_domain::{InitiativeId, InitiativeName};
    use kanban_dto::{TimelineEntityKind, TimelineEntityRef, TimelineEventKind};
    use rusqlite::{Connection, params};
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

    /// Keep the `retained` most recent outcomes, optionally with a
    /// minimum age.
    fn keep(retained: u32) -> RetentionPolicy {
        RetentionPolicy::keep_most_recent(NonZeroU32::new(retained).expect("the bound is not zero"))
    }

    /// Keep the `retained` most recent outcomes and every outcome
    /// younger than `minimum_age_secs`.
    fn policy(retained: u32, minimum_age_secs: u64) -> RetentionPolicy {
        RetentionPolicy::new(
            NonZeroU32::new(retained).expect("the bound is not zero"),
            Duration::from_secs(minimum_age_secs),
        )
    }

    /// Insert one outcome directly, backdating its recorded time.
    fn insert_outcome(conn: &Connection, key: &str, label: &str, recorded_age_secs: i64) {
        let recorded = outcome(label);
        let response = serde_json::to_string(&recorded.response).expect("the response encodes");
        conn.execute(
            "INSERT INTO idempotency_outcomes (idempotency_key, fingerprint, response, recorded_at)
             VALUES (?1, ?2, ?3, strftime('%Y-%m-%dT%H:%M:%fZ', 'now', ?4))",
            params![
                key,
                recorded.fingerprint,
                response,
                format!("-{recorded_age_secs} seconds"),
            ],
        )
        .expect("the row inserts");
    }

    /// Every key in `keys` that still has a recorded outcome.
    fn spent_keys<'a>(store: &SqliteIdempotencyStore, keys: &'a [&'a str]) -> Vec<&'a str> {
        keys.iter()
            .filter(|key| store.recorded(key).expect("the lookup serves").is_some())
            .copied()
            .collect()
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
                &|id: InitiativeId| {
                    TimelineEnvelope::global(
                        TimelineEventKind::Transition,
                        Some(TimelineEntityRef {
                            kind: TimelineEntityKind::Initiative,
                            id: id.value().to_string(),
                        }),
                        json!({ "action": "created", "id": id.value(), "name": name }),
                    )
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
    fn retention_keeps_outcomes_within_the_minimum_age_even_when_the_count_bound_is_exceeded() {
        let (_dir, database) = migrated();
        let conn = database.connection();
        let keys: Vec<String> = (1..=12).map(|index| format!("key-{index}")).collect();
        for (index, key) in keys.iter().enumerate() {
            insert_outcome(&conn, key, &format!("Outcome-{index}"), 1_800);
        }

        let store = SqliteIdempotencyStore::new(&database, policy(2, 3_600));
        store
            .begin()
            .expect("the span opens")
            .commit("trigger", outcome("trigger"))
            .expect("the span commits");

        let key_refs: Vec<&str> = keys.iter().map(String::as_str).collect();
        assert_eq!(
            spent_keys(&store, &key_refs),
            key_refs,
            "every outcome inside the minimum age survives the burst"
        );
        assert!(
            store
                .recorded("trigger")
                .expect("the lookup serves")
                .is_some(),
            "the commit that triggered pruning keeps its own outcome"
        );
    }

    #[test]
    fn retention_prunes_outcomes_beyond_both_the_count_bound_and_minimum_age() {
        let (_dir, database) = migrated();
        let conn = database.connection();
        for index in 1..=8 {
            insert_outcome(
                &conn,
                &format!("old-{index}"),
                &format!("Old-{index}"),
                7_200,
            );
        }
        for index in 1..=3 {
            insert_outcome(&conn, &format!("new-{index}"), &format!("New-{index}"), 0);
        }

        let store = SqliteIdempotencyStore::new(&database, policy(2, 3_600));
        store
            .begin()
            .expect("the span opens")
            .commit("trigger", outcome("trigger"))
            .expect("the span commits");

        for index in 1..=8 {
            assert!(
                store
                    .recorded(&format!("old-{index}"))
                    .expect("the lookup serves")
                    .is_none(),
                "outcomes older than the minimum age are pruned"
            );
        }
        assert_eq!(
            spent_keys(&store, &["new-1", "new-2", "new-3", "trigger"],),
            vec!["new-1", "new-2", "new-3", "trigger"],
            "recent outcomes survive even when they exceed the count bound"
        );
    }

    #[test]
    fn retention_prunes_deterministically_across_repeated_bursts() {
        let (_dir, database) = migrated();
        let conn = database.connection();
        for index in 1..=6 {
            insert_outcome(
                &conn,
                &format!("old-{index}"),
                &format!("Old-{index}"),
                7_200,
            );
        }
        for index in 1..=4 {
            insert_outcome(&conn, &format!("new-{index}"), &format!("New-{index}"), 0);
        }

        let store = SqliteIdempotencyStore::new(&database, policy(2, 3_600));
        for label in ["first-trigger", "second-trigger"] {
            store
                .begin()
                .expect("the span opens")
                .commit(label, outcome(label))
                .expect("the span commits");
        }

        let survivors = spent_keys(
            &store,
            &[
                "old-1",
                "old-2",
                "old-3",
                "old-4",
                "old-5",
                "old-6",
                "new-1",
                "new-2",
                "new-3",
                "new-4",
                "first-trigger",
                "second-trigger",
            ],
        );
        assert_eq!(
            survivors,
            vec![
                "new-1",
                "new-2",
                "new-3",
                "new-4",
                "first-trigger",
                "second-trigger"
            ],
            "repeated pruning leaves the same survivors"
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
