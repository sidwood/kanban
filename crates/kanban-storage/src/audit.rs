//! The append-only audit trail.
//!
//! `audit_events` accepts inserts and nothing else (ADR-0002,
//! DR-SS-04). This module is the only writer.

use rusqlite::Connection;
use serde_json::Value;

use crate::error::StorageError;

/// Appends one audit event. There is no update or delete path.
pub(crate) fn insert_event(
    conn: &Connection,
    kind: &str,
    detail: &Value,
) -> Result<(), StorageError> {
    conn.execute(
        "INSERT INTO audit_events (kind, detail) VALUES (?1, ?2)",
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
            .append_audit_event("probe.kind", &json!({ "probe": true }))
            .expect("the append path inserts");
        database
    }

    #[test]
    fn appending_through_the_api_succeeds() {
        let database = seeded_database();

        let rows: i64 = database
            .connection()
            .query_row("SELECT COUNT(*) FROM audit_events", [], |row| row.get(0))
            .expect("the audit trail is readable");
        assert_eq!(rows, 17, "one event per migration, plus the probe");
    }

    #[test]
    fn updating_audit_events_fails() {
        let database = seeded_database();

        let outcome = database
            .connection()
            .execute("UPDATE audit_events SET kind = 'tampered'", []);

        let error = outcome.expect_err("the schema must refuse updates");
        assert!(
            error.to_string().contains("append-only"),
            "the refusal should say append-only, got: {error}"
        );
    }

    #[test]
    fn deleting_audit_events_fails() {
        let database = seeded_database();

        let outcome = database
            .connection()
            .execute("DELETE FROM audit_events", []);

        let error = outcome.expect_err("the schema must refuse deletes");
        assert!(
            error.to_string().contains("append-only"),
            "the refusal should say append-only, got: {error}"
        );
    }

    #[test]
    fn replacing_audit_events_fails() {
        let database = seeded_database();

        let outcome = database.connection().execute(
            "INSERT OR REPLACE INTO audit_events (id, kind, detail) VALUES (1, 'tampered', '{}')",
            [],
        );

        let error = outcome.expect_err("the schema must refuse replaces");
        assert!(
            error.to_string().contains("append-only"),
            "the refusal should say append-only, got: {error}"
        );
        let kind: String = database
            .connection()
            .query_row("SELECT kind FROM audit_events WHERE id = 1", [], |row| {
                row.get(0)
            })
            .expect("the original row is still readable");
        assert_eq!(kind, "migration.applied", "the row must not be mutated");
    }
}
