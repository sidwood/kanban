//! The append-only activity timeline.
//!
//! `timeline_events` accepts inserts and nothing else (ADR-0002).
//! This module writes the application's typed envelope unchanged and
//! serves filtered queries back to the application layer.

use kanban_app::TimelineEnvelope;
use kanban_dto::TimelineScope;
use rusqlite::{Connection, Row, params};
use serde_json::Value;

use crate::error::StorageError;

/// One timeline row as stored: the scope columns and the vocabulary
/// as text. Decoding them back into the payload types is the
/// service adapter's job, so corruption is refused in one place.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimelineRow {
    pub id: u64,
    pub scope: String,
    pub project_id: String,
    pub kind: String,
    pub entity_kind: Option<String>,
    pub entity_id: Option<String>,
    pub recorded_at: String,
    pub detail: Value,
}

/// Filters for the timeline query surface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimelineFilter {
    pub scope: TimelineScope,
    pub entity_kind: Option<String>,
    pub entity_id: Option<String>,
    pub kinds: Vec<String>,
    pub since: Option<String>,
    pub until: Option<String>,
}

impl TimelineFilter {
    /// An unfiltered read of one scope.
    pub fn of(scope: TimelineScope) -> Self {
        Self {
            scope,
            entity_kind: None,
            entity_id: None,
            kinds: Vec::new(),
            since: None,
            until: None,
        }
    }
}

/// The scope's two stored columns: which scope, and the Project it
/// names when it names one.
fn scope_columns(scope: &TimelineScope) -> (&'static str, &str) {
    match scope {
        TimelineScope::Global => ("global", ""),
        TimelineScope::Project(project_id) => ("project", project_id.as_str()),
    }
}

/// Appends one timeline event. There is no update or delete path.
pub(crate) fn insert_event(
    conn: &Connection,
    event: &TimelineEnvelope,
) -> Result<(), StorageError> {
    let (scope, project_id) = scope_columns(event.scope());
    conn.execute(
        "INSERT INTO timeline_events
             (scope, project_id, kind, entity_kind, entity_id, detail)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            scope,
            project_id,
            event.kind().as_str(),
            event.entity().map(|entity| entity.kind.as_str()),
            event.entity().map(|entity| entity.id.as_str()),
            event.detail().to_string(),
        ],
    )?;
    Ok(())
}

/// Returns timeline rows for `filter`, oldest first.
pub(crate) fn query_events(
    conn: &Connection,
    filter: &TimelineFilter,
) -> Result<Vec<TimelineRow>, StorageError> {
    let (scope, project_id) = scope_columns(&filter.scope);
    if matches!(filter.scope, TimelineScope::Project(_)) && project_id.is_empty() {
        return Err(StorageError::InvalidTimeline {
            reason: "a Project timeline scope must name a Project".to_owned(),
        });
    }

    let mut sql = String::from(
        "SELECT id, scope, project_id, kind, entity_kind, entity_id, recorded_at, detail
         FROM timeline_events
         WHERE scope = ?1 AND project_id = ?2",
    );
    let mut bindings: Vec<String> = vec![scope.to_owned(), project_id.to_owned()];

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
    let detail_text: String = row.get(7)?;
    let detail = serde_json::from_str(&detail_text).map_err(|_| {
        rusqlite::Error::InvalidColumnType(7, "detail".to_owned(), rusqlite::types::Type::Text)
    })?;
    Ok(TimelineRow {
        id: row.get::<_, i64>(0)? as u64,
        scope: row.get(1)?,
        project_id: row.get(2)?,
        kind: row.get(3)?,
        entity_kind: row.get(4)?,
        entity_id: row.get(5)?,
        recorded_at: row.get(6)?,
        detail,
    })
}

#[cfg(test)]
mod query_filters {
    use kanban_app::TimelineEnvelope;
    use kanban_dto::{TimelineEntityKind, TimelineEntityRef, TimelineEventKind, TimelineScope};
    use serde_json::json;

    use super::TimelineFilter;
    use crate::migrations::AllowAllMigrations;
    use crate::test_support::scratch_database;

    fn migrated_database() -> crate::db::Database {
        let (_dir, mut database) = scratch_database();
        database
            .migrate(&AllowAllMigrations)
            .expect("migrations apply");
        database
    }

    fn entity(kind: TimelineEntityKind, id: &str) -> TimelineEntityRef {
        TimelineEntityRef {
            kind,
            id: id.to_owned(),
        }
    }

    fn append(database: &crate::db::Database, envelope: TimelineEnvelope) {
        database
            .append_timeline_event(&envelope)
            .expect("the append path inserts");
    }

    fn ticket_event(
        project_id: &str,
        kind: TimelineEventKind,
        ticket: &str,
        detail: serde_json::Value,
    ) -> TimelineEnvelope {
        TimelineEnvelope::project(
            project_id,
            kind,
            Some(entity(TimelineEntityKind::Ticket, ticket)),
            detail,
        )
        .expect("a named Project is accepted")
    }

    #[test]
    fn the_envelope_is_stored_unchanged() {
        let database = migrated_database();
        append(
            &database,
            ticket_event(
                "kan",
                TimelineEventKind::Transition,
                "kan-t9",
                json!({ "action": "started", "to": "in_progress" }),
            ),
        );

        let rows = database
            .query_timeline(&TimelineFilter::of(TimelineScope::Project(
                "kan".to_owned(),
            )))
            .expect("the timeline is readable");

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].scope, "project");
        assert_eq!(rows[0].project_id, "kan");
        assert_eq!(rows[0].kind, "transition");
        assert_eq!(rows[0].entity_kind.as_deref(), Some("ticket"));
        assert_eq!(rows[0].entity_id.as_deref(), Some("kan-t9"));
        assert_eq!(
            rows[0].detail,
            json!({ "action": "started", "to": "in_progress" }),
            "storage must not touch the facts the application stated"
        );
    }

    #[test]
    fn a_global_event_is_queryable_in_the_global_scope() {
        let database = migrated_database();
        append(
            &database,
            TimelineEnvelope::global(
                TimelineEventKind::Transition,
                Some(entity(TimelineEntityKind::Initiative, "1")),
                json!({ "action": "created", "id": 1 }),
            ),
        );
        append(
            &database,
            ticket_event(
                "kan",
                TimelineEventKind::Transition,
                "kan-t9",
                json!({ "action": "started" }),
            ),
        );

        let global = database
            .query_timeline(&TimelineFilter::of(TimelineScope::Global))
            .expect("the global timeline is readable");
        let project = database
            .query_timeline(&TimelineFilter::of(TimelineScope::Project(
                "kan".to_owned(),
            )))
            .expect("the Project timeline is readable");

        assert_eq!(global.len(), 1);
        assert_eq!(global[0].entity_kind.as_deref(), Some("initiative"));
        assert_eq!(project.len(), 1, "the scopes do not leak into each other");
        assert_eq!(project[0].entity_id.as_deref(), Some("kan-t9"));
    }

    #[test]
    fn a_project_scope_without_an_identity_is_refused() {
        let database = migrated_database();

        let error = database
            .query_timeline(&TimelineFilter::of(TimelineScope::Project(String::new())))
            .expect_err("a Project scope must name a Project");

        assert!(matches!(
            error,
            crate::error::StorageError::InvalidTimeline { .. }
        ));
    }

    #[test]
    fn query_filters_by_entity_kind_and_id() {
        let database = migrated_database();
        append(
            &database,
            ticket_event(
                "kan",
                TimelineEventKind::Transition,
                "kan-t9",
                json!({ "to": "in_progress" }),
            ),
        );
        append(
            &database,
            ticket_event(
                "kan",
                TimelineEventKind::Comment,
                "kan-t10",
                json!({ "text": "elsewhere" }),
            ),
        );

        let rows = database
            .query_timeline(&TimelineFilter {
                entity_kind: Some("ticket".to_owned()),
                entity_id: Some("kan-t9".to_owned()),
                ..TimelineFilter::of(TimelineScope::Project("kan".to_owned()))
            })
            .expect("entity filter applies");

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].kind, "transition");
    }

    #[test]
    fn query_filters_by_kind_and_time_window() {
        let database = migrated_database();
        for (kind, recorded_at) in [
            ("run", "2026-03-01T00:00:00.000000Z"),
            ("telemetry", "2026-06-01T00:00:00.000000Z"),
            ("review", "2026-12-01T00:00:00.000000Z"),
        ] {
            database
                .connection()
                .execute(
                    "INSERT INTO timeline_events
                     (scope, project_id, kind, entity_kind, entity_id, detail, recorded_at)
                     VALUES ('project', 'kan', ?1, 'ticket', 'kan-t9', '{}', ?2)",
                    rusqlite::params![kind, recorded_at],
                )
                .expect("fixture row lands");
        }

        let rows = database
            .query_timeline(&TimelineFilter {
                kinds: vec!["run".to_owned(), "review".to_owned()],
                since: Some("2026-02-01T00:00:00.000000Z".to_owned()),
                until: Some("2026-11-01T00:00:00.000000Z".to_owned()),
                ..TimelineFilter::of(TimelineScope::Project("kan".to_owned()))
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
            ticket_event(
                "kan",
                TimelineEventKind::Transition,
                "kan-t9",
                json!({ "to": "done" }),
            ),
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
                entity_kind: Some("ticket".to_owned()),
                entity_id: Some("kan-t9".to_owned()),
                ..TimelineFilter::of(TimelineScope::Project("kan".to_owned()))
            })
            .expect("archived entities remain queryable");

        assert_eq!(rows.len(), 1, "archiving must not touch timeline rows");
    }
}

#[cfg(test)]
mod scope_migration {
    use kanban_dto::{TimelineEntityKind, TimelineScope};
    use serde_json::json;

    use super::TimelineFilter;
    use crate::migrations::{AllowAllMigrations, apply_through};
    use crate::test_support::scratch_database;

    /// A database on the schema that wrote Initiative history with an
    /// empty Project identity and uncatalogued `initiative.*` kinds.
    fn database_with_unreachable_initiative_history() -> crate::db::Database {
        let (_dir, database) = scratch_database();
        apply_through(&database.connection(), 5).expect("the older schema applies");
        for (kind, detail, recorded_at) in [
            (
                "initiative.created",
                json!({ "name": "Reliability", "id": 1 }),
                "2026-03-01T00:00:00.000000Z",
            ),
            (
                "initiative.renamed",
                json!({ "from": "Reliability", "to": "Recovery", "id": 1 }),
                "2026-04-01T00:00:00.000000Z",
            ),
            (
                "initiative.archived",
                json!({ "id": 1 }),
                "2026-05-01T00:00:00.000000Z",
            ),
        ] {
            database
                .connection()
                .execute(
                    "INSERT INTO timeline_events
                     (project_id, kind, entity_kind, entity_id, detail, recorded_at)
                     VALUES ('', ?1, 'initiative', '1', ?2, ?3)",
                    rusqlite::params![kind, detail.to_string(), recorded_at],
                )
                .expect("the legacy row lands");
        }
        database
    }

    #[test]
    fn the_migration_repairs_initiative_history_without_losing_anything() {
        let mut database = database_with_unreachable_initiative_history();

        database
            .migrate(&AllowAllMigrations)
            .expect("the scope migration applies");

        let rows = database
            .query_timeline(&TimelineFilter {
                entity_kind: Some(TimelineEntityKind::Initiative.as_str().to_owned()),
                entity_id: Some("1".to_owned()),
                ..TimelineFilter::of(TimelineScope::Global)
            })
            .expect("repaired Initiative history is queryable");

        assert_eq!(
            rows.iter()
                .map(|row| (
                    row.id,
                    row.kind.clone(),
                    row.recorded_at.clone(),
                    row.detail.clone()
                ))
                .collect::<Vec<_>>(),
            vec![
                (
                    1,
                    "transition".to_owned(),
                    "2026-03-01T00:00:00.000000Z".to_owned(),
                    json!({ "name": "Reliability", "id": 1, "action": "created" }),
                ),
                (
                    2,
                    "transition".to_owned(),
                    "2026-04-01T00:00:00.000000Z".to_owned(),
                    json!({
                        "from": "Reliability",
                        "to": "Recovery",
                        "id": 1,
                        "action": "renamed",
                    }),
                ),
                (
                    3,
                    "transition".to_owned(),
                    "2026-05-01T00:00:00.000000Z".to_owned(),
                    json!({ "id": 1, "action": "archived" }),
                ),
            ],
            "each row keeps its identity, its time, and every fact it carried"
        );
    }

    #[test]
    fn the_migration_leaves_project_history_where_it_was() {
        let (_dir, mut database) = scratch_database();
        apply_through(&database.connection(), 5).expect("the older schema applies");
        database
            .connection()
            .execute(
                "INSERT INTO timeline_events (project_id, kind, detail)
                 VALUES ('kan', 'comment', '{\"text\":\"noted\"}')",
                [],
            )
            .expect("the legacy row lands");

        database
            .migrate(&AllowAllMigrations)
            .expect("the scope migration applies");

        let rows = database
            .query_timeline(&TimelineFilter::of(TimelineScope::Project(
                "kan".to_owned(),
            )))
            .expect("Project history is queryable");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].scope, "project");
        assert_eq!(rows[0].kind, "comment");
    }

    #[test]
    fn the_repair_leaves_no_update_path_behind() {
        let mut database = database_with_unreachable_initiative_history();
        database
            .migrate(&AllowAllMigrations)
            .expect("the scope migration applies");

        let outcome = database
            .connection()
            .execute("UPDATE timeline_events SET kind = 'tampered'", []);

        let error = outcome.expect_err("the schema must refuse updates again");
        assert!(
            error.to_string().contains("append-only"),
            "the refusal should say append-only, got: {error}"
        );
    }
}

#[cfg(test)]
mod append_only {
    use kanban_app::TimelineEnvelope;
    use kanban_dto::TimelineEventKind;
    use serde_json::json;

    use crate::migrations::AllowAllMigrations;
    use crate::test_support::scratch_database;

    fn seeded_database() -> crate::db::Database {
        let (_dir, mut database) = scratch_database();
        database
            .migrate(&AllowAllMigrations)
            .expect("the initial migration applies");
        database
            .append_timeline_event(&TimelineEnvelope::global(
                TimelineEventKind::Telemetry,
                None,
                json!({ "probe": true }),
            ))
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
        assert_eq!(kind, "telemetry", "the row must not be mutated");
    }
}
