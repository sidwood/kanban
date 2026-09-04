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
