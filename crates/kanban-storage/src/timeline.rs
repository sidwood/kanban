//! The append-only activity timeline.
//!
//! `timeline_events` accepts inserts and nothing else (ADR-0002).
//! This module appends the per-Project event envelope and serves
//! filtered queries for the application layer.

use rusqlite::{Connection, Row, params};
use serde_json::Value;

use crate::error::StorageError;

/// The inputs for one timeline append.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimelineAppend {
    pub project_id: String,
    pub kind: String,
    pub entity_kind: Option<String>,
    pub entity_id: Option<String>,
    pub detail: Value,
}

/// One timeline row returned from queries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimelineRow {
    pub id: u64,
    pub project_id: String,
    pub kind: String,
    pub entity_kind: Option<String>,
    pub entity_id: Option<String>,
    pub recorded_at: String,
    pub detail: Value,
}

/// Filters for the timeline query surface.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TimelineFilter {
    pub project_id: String,
    pub entity_kind: Option<String>,
    pub entity_id: Option<String>,
    pub kinds: Vec<String>,
    pub since: Option<String>,
    pub until: Option<String>,
}

/// Appends one timeline event. There is no update or delete path.
pub(crate) fn insert_event(conn: &Connection, event: &TimelineAppend) -> Result<(), StorageError> {
    conn.execute(
        "INSERT INTO timeline_events (project_id, kind, entity_kind, entity_id, detail)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            event.project_id,
            event.kind,
            event.entity_kind,
            event.entity_id,
            event.detail.to_string(),
        ],
    )?;
    Ok(())
}

/// Returns timeline rows for `filter`, oldest first.
pub(crate) fn query_events(
    conn: &Connection,
    filter: &TimelineFilter,
) -> Result<Vec<TimelineRow>, StorageError> {
    if filter.project_id.is_empty() {
        return Err(StorageError::InvalidTimeline {
            reason: "project_id is required".to_owned(),
        });
    }

    let mut sql = String::from(
        "SELECT id, project_id, kind, entity_kind, entity_id, recorded_at, detail
         FROM timeline_events
         WHERE project_id = ?1",
    );
    let mut bindings: Vec<String> = vec![filter.project_id.clone()];

    if let Some(entity_kind) = &filter.entity_kind {
        sql.push_str(" AND entity_kind = ?");
        bindings.push(entity_kind.clone());
    }
    if let Some(entity_id) = &filter.entity_id {
        sql.push_str(" AND entity_id = ?");
        bindings.push(entity_id.clone());
    }
    if !filter.kinds.is_empty() {
        let placeholders = (0..filter.kinds.len())
            .map(|_| "?")
            .collect::<Vec<_>>()
            .join(", ");
        sql.push_str(&format!(" AND kind IN ({placeholders})"));
        bindings.extend(filter.kinds.iter().cloned());
    }
    if filter.since.is_some() {
        sql.push_str(" AND recorded_at >= ?");
        bindings.push(filter.since.clone().expect("checked above"));
    }
    if filter.until.is_some() {
        sql.push_str(" AND recorded_at <= ?");
        bindings.push(filter.until.clone().expect("checked above"));
    }
    sql.push_str(" ORDER BY recorded_at ASC, id ASC");

    let mut statement = conn.prepare(&sql)?;
    let rows = statement
        .query_map(rusqlite::params_from_iter(bindings.iter()), decode_row)?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

fn decode_row(row: &Row<'_>) -> Result<TimelineRow, rusqlite::Error> {
    let detail_text: String = row.get(6)?;
    let detail = serde_json::from_str(&detail_text).map_err(|_| {
        rusqlite::Error::InvalidColumnType(6, "detail".to_owned(), rusqlite::types::Type::Text)
    })?;
    Ok(TimelineRow {
        id: row.get::<_, i64>(0)? as u64,
        project_id: row.get(1)?,
        kind: row.get(2)?,
        entity_kind: row.get(3)?,
        entity_id: row.get(4)?,
        recorded_at: row.get(5)?,
        detail,
    })
}

#[cfg(test)]
mod query_filters {
    use serde_json::json;

    use super::{TimelineAppend, TimelineFilter};
    use crate::migrations::AllowAllMigrations;
    use crate::test_support::scratch_database;

    fn migrated_database() -> crate::db::Database {
        let (_dir, mut database) = scratch_database();
        database
            .migrate(&AllowAllMigrations)
            .expect("migrations apply");
        database
    }

    fn append(
        database: &crate::db::Database,
        project_id: &str,
        kind: &str,
        entity_kind: Option<&str>,
        entity_id: Option<&str>,
        detail: &serde_json::Value,
    ) {
        database
            .append_timeline_event(&TimelineAppend {
                project_id: project_id.to_owned(),
                kind: kind.to_owned(),
                entity_kind: entity_kind.map(str::to_owned),
                entity_id: entity_id.map(str::to_owned),
                detail: detail.clone(),
            })
            .expect("the append path inserts");
    }

    #[test]
    fn appending_through_the_api_succeeds() {
        let database = migrated_database();
        append(
            &database,
            "kan",
            "transition",
            Some("ticket"),
            Some("kan-t9"),
            &json!({ "to": "in_progress" }),
        );

        let rows = database
            .query_timeline(&TimelineFilter {
                project_id: "kan".to_owned(),
                ..TimelineFilter::default()
            })
            .expect("the timeline is readable");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].kind, "transition");
    }

    #[test]
    fn query_filters_by_entity_kind_and_id() {
        let database = migrated_database();
        append(
            &database,
            "kan",
            "transition",
            Some("ticket"),
            Some("kan-t9"),
            &json!({ "to": "in_progress" }),
        );
        append(
            &database,
            "kan",
            "comment",
            Some("ticket"),
            Some("kan-t10"),
            &json!({ "text": "elsewhere" }),
        );

        let rows = database
            .query_timeline(&TimelineFilter {
                project_id: "kan".to_owned(),
                entity_kind: Some("ticket".to_owned()),
                entity_id: Some("kan-t9".to_owned()),
                ..TimelineFilter::default()
            })
            .expect("entity filter applies");

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].kind, "transition");
    }

    #[test]
    fn query_filters_by_kind_and_time_window() {
        let database = migrated_database();
        database
            .connection()
            .execute(
                "INSERT INTO timeline_events
                 (project_id, kind, entity_kind, entity_id, detail, recorded_at)
                 VALUES ('kan', 'run', 'ticket', 'kan-t9', '{}', '2026-03-01T00:00:00.000000Z')",
                [],
            )
            .expect("fixture row lands");
        database
            .connection()
            .execute(
                "INSERT INTO timeline_events
                 (project_id, kind, entity_kind, entity_id, detail, recorded_at)
                 VALUES ('kan', 'telemetry', 'ticket', 'kan-t9', '{}', '2026-06-01T00:00:00.000000Z')",
                [],
            )
            .expect("fixture row lands");
        database
            .connection()
            .execute(
                "INSERT INTO timeline_events
                 (project_id, kind, entity_kind, entity_id, detail, recorded_at)
                 VALUES ('kan', 'review', 'ticket', 'kan-t9', '{}', '2026-12-01T00:00:00.000000Z')",
                [],
            )
            .expect("fixture row lands");

        let rows = database
            .query_timeline(&TimelineFilter {
                project_id: "kan".to_owned(),
                kinds: vec!["run".to_owned(), "review".to_owned()],
                since: Some("2026-02-01T00:00:00.000000Z".to_owned()),
                until: Some("2026-11-01T00:00:00.000000Z".to_owned()),
                ..TimelineFilter::default()
            })
            .expect("kind and time filters apply");

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].kind, "run");
    }

    #[test]
    fn archived_entities_leave_timelines_intact_and_queryable() {
        let database = migrated_database();
        append(
            &database,
            "kan",
            "transition",
            Some("ticket"),
            Some("kan-t9"),
            &json!({ "to": "done" }),
        );
        database
            .connection()
            .execute(
                "CREATE TABLE archived_entities (
                     entity_kind TEXT NOT NULL,
                     entity_id TEXT NOT NULL,
                     PRIMARY KEY (entity_kind, entity_id)
                 )",
                [],
            )
            .expect("archival marker table lands");
        database
            .connection()
            .execute(
                "INSERT INTO archived_entities (entity_kind, entity_id) VALUES ('ticket', 'kan-t9')",
                [],
            )
            .expect("the entity is archived");

        let rows = database
            .query_timeline(&TimelineFilter {
                project_id: "kan".to_owned(),
                entity_kind: Some("ticket".to_owned()),
                entity_id: Some("kan-t9".to_owned()),
                ..TimelineFilter::default()
            })
            .expect("archived entities remain queryable");

        assert_eq!(rows.len(), 1, "archiving must not touch timeline rows");
    }
}

#[cfg(test)]
mod append_only {
    use serde_json::json;

    use super::TimelineAppend;
    use crate::migrations::AllowAllMigrations;
    use crate::test_support::scratch_database;

    fn seeded_database() -> crate::db::Database {
        let (_dir, mut database) = scratch_database();
        database
            .migrate(&AllowAllMigrations)
            .expect("the initial migration applies");
        database
            .append_timeline_event(&TimelineAppend {
                project_id: "kan".to_owned(),
                kind: "probe.kind".to_owned(),
                entity_kind: None,
                entity_id: None,
                detail: json!({ "probe": true }),
            })
            .expect("the append path inserts");
        database
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

    #[test]
    fn replacing_timeline_events_fails() {
        let database = seeded_database();

        let outcome = database.connection().execute(
            "INSERT OR REPLACE INTO timeline_events (id, project_id, kind, detail) VALUES (1, 'kan', 'tampered', '{}')",
            [],
        );

        let error = outcome.expect_err("the schema must refuse replaces");
        assert!(
            error.to_string().contains("append-only"),
            "the refusal should say append-only, got: {error}"
        );
        let kind: String = database
            .connection()
            .query_row("SELECT kind FROM timeline_events WHERE id = 1", [], |row| {
                row.get(0)
            })
            .expect("the original row is still readable");
        assert_eq!(kind, "probe.kind", "the row must not be mutated");
    }
}
