//! The append-only activity timeline.
//!
//! `timeline_events` accepts inserts and nothing else (ADR-0002).
//! This module is the only writer; KAN-S2 builds the queries and
//! surfaces above it.

use rusqlite::Connection;
use serde_json::Value;

use crate::error::StorageError;

/// Appends one timeline event. There is no update or delete path.
pub(crate) fn insert_event(
    conn: &Connection,
    kind: &str,
    detail: &Value,
) -> Result<(), StorageError> {
    conn.execute(
        "INSERT INTO timeline_events (kind, detail) VALUES (?1, ?2)",
        rusqlite::params![kind, detail.to_string()],
    )?;
    Ok(())
}

#[cfg(test)]
mod append_only {
    use serde_json::json;

    use crate::migrations::AllowAllMigrations;
    use crate::test_support::scratch_database;

    fn seeded_database() -> crate::db::Database {
        let (_dir, mut database) = scratch_database();
        database
            .migrate(&AllowAllMigrations)
            .expect("the initial migration applies");
        database
            .append_timeline_event("probe.kind", &json!({ "probe": true }))
            .expect("the append path inserts");
        database
    }

    #[test]
    fn appending_through_the_api_succeeds() {
        let database = seeded_database();

        let rows: i64 = database
            .connection()
            .query_row("SELECT COUNT(*) FROM timeline_events", [], |row| row.get(0))
            .expect("the timeline is readable");
        assert_eq!(rows, 1, "timeline events are not auto-written");
    }

    #[test]
    fn updating_timeline_events_fails() {
        let database = seeded_database();

        let outcome = database
            .connection()
            .execute("UPDATE timeline_events SET kind = 'tampered'", []);

        let error = outcome.expect_err("the schema must refuse updates");
        assert!(
            error.to_string().contains("append-only"),
            "the refusal should say append-only, got: {error}"
        );
    }

    #[test]
    fn deleting_timeline_events_fails() {
        let database = seeded_database();

        let outcome = database
            .connection()
            .execute("DELETE FROM timeline_events", []);

        let error = outcome.expect_err("the schema must refuse deletes");
        assert!(
            error.to_string().contains("append-only"),
            "the refusal should say append-only, got: {error}"
        );
    }
}
