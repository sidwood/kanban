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

/// The scope's two stored columns: which scope, and the decimal text
/// of the numeric Project identity it names when it names one. The
/// column stays TEXT because legacy rows hold arbitrary strings until
/// the identity migration reunifies them.
fn scope_columns(scope: &TimelineScope) -> (&'static str, String) {
    match scope {
        TimelineScope::Global => ("global", String::new()),
        TimelineScope::Project(project_id) => ("project", project_id.to_string()),
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
    use kanban_domain::{TimelineTimeError, validate_timeline_time_window};
    use kanban_dto::{TimelineEntityKind, TimelineEntityRef, TimelineEventKind, TimelineScope};
    use serde_json::json;
    use time::format_description::well_known::Rfc3339;
    use time::{Duration, OffsetDateTime, UtcOffset};

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
        project_id: u64,
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
    }

    /// Appends one event and returns the instant storage gave it.
    /// Bounds are derived from these instants so every boundary test
    /// faces the shape production actually writes.
    fn append_and_record(database: &crate::db::Database, envelope: TimelineEnvelope) -> String {
        append(database, envelope);
        database
            .query_timeline(&TimelineFilter::of(TimelineScope::Project(1)))
            .expect("appended rows are readable")
            .last()
            .expect("the append landed")
            .recorded_at
            .clone()
    }

    /// One recorded instant at another offset, the way a client in
    /// that timezone names the same moment.
    fn at_offset(recorded_at: &str, offset_hours: i8) -> String {
        OffsetDateTime::parse(recorded_at, &Rfc3339)
            .expect("stored timestamps parse as RFC 3339")
            .to_offset(UtcOffset::from_hms(offset_hours, 0, 0).expect("the offset is valid"))
            .format(&Rfc3339)
            .expect("the offset instant formats as RFC 3339")
    }

    /// One recorded instant shifted by whole seconds.
    fn shifted_by_seconds(recorded_at: &str, seconds: i64) -> String {
        OffsetDateTime::parse(recorded_at, &Rfc3339)
            .expect("stored timestamps parse as RFC 3339")
            .checked_add(Duration::seconds(seconds))
            .expect("the shifted instant stays in range")
            .format(&Rfc3339)
            .expect("the shifted instant formats as RFC 3339")
    }

    /// One recorded instant with an extra fractional digit, finer
    /// than the stored millisecond: `...123Z` becomes `...1239Z`.
    fn finer_than_the_stored_millisecond(recorded_at: &str) -> String {
        format!(
            "{}9Z",
            recorded_at
                .strip_suffix('Z')
                .expect("stored timestamps end in Z")
        )
    }

    #[test]
    fn the_envelope_is_stored_unchanged() {
        let database = migrated_database();
        append(
            &database,
            ticket_event(
                1,
                TimelineEventKind::Transition,
                "kan-t9",
                json!({ "action": "started", "to": "in_progress" }),
            ),
        );

        let rows = database
            .query_timeline(&TimelineFilter::of(TimelineScope::Project(1)))
            .expect("the timeline is readable");

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].scope, "project");
        assert_eq!(rows[0].project_id, "1");
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
                1,
                TimelineEventKind::Transition,
                "kan-t9",
                json!({ "action": "started" }),
            ),
        );

        let global = database
            .query_timeline(&TimelineFilter::of(TimelineScope::Global))
            .expect("the global timeline is readable");
        let project = database
            .query_timeline(&TimelineFilter::of(TimelineScope::Project(1)))
            .expect("the Project timeline is readable");

        assert_eq!(global.len(), 1);
        assert_eq!(global[0].entity_kind.as_deref(), Some("initiative"));
        assert_eq!(project.len(), 1, "the scopes do not leak into each other");
        assert_eq!(project[0].entity_id.as_deref(), Some("kan-t9"));
    }

    #[test]
    fn query_filters_by_entity_kind_and_id() {
        let database = migrated_database();
        append(
            &database,
            ticket_event(
                1,
                TimelineEventKind::Transition,
                "kan-t9",
                json!({ "to": "in_progress" }),
            ),
        );
        append(
            &database,
            ticket_event(
                1,
                TimelineEventKind::Comment,
                "kan-t10",
                json!({ "text": "elsewhere" }),
            ),
        );

        let rows = database
            .query_timeline(&TimelineFilter {
                entity_kind: Some("ticket".to_owned()),
                entity_id: Some("kan-t9".to_owned()),
                ..TimelineFilter::of(TimelineScope::Project(1))
            })
            .expect("entity filter applies");

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].kind, "transition");
    }

    #[test]
    fn query_filters_by_kind() {
        let database = migrated_database();
        for kind in [
            TimelineEventKind::Run,
            TimelineEventKind::Telemetry,
            TimelineEventKind::Review,
        ] {
            append(
                &database,
                ticket_event(1, kind, "kan-t9", json!({ "recorded": true })),
            );
        }

        let rows = database
            .query_timeline(&TimelineFilter {
                kinds: vec![
                    TimelineEventKind::Run.as_str().to_owned(),
                    TimelineEventKind::Review.as_str().to_owned(),
                ],
                ..TimelineFilter::of(TimelineScope::Project(1))
            })
            .expect("the kind filter applies");

        assert_eq!(
            rows.iter().map(|row| row.kind.as_str()).collect::<Vec<_>>(),
            vec!["run", "review"],
            "only the requested kinds are served, oldest first"
        );
    }

    #[test]
    fn an_until_bound_equal_to_a_row_instant_includes_the_row() {
        let database = migrated_database();
        let recorded_at = append_and_record(
            &database,
            ticket_event(
                1,
                TimelineEventKind::Transition,
                "kan-t9",
                json!({ "to": "in_progress" }),
            ),
        );

        let (_, until) = validate_timeline_time_window(None, Some(&recorded_at))
            .expect("a stored instant is a valid until bound");

        let rows = database
            .query_timeline(&TimelineFilter {
                until,
                ..TimelineFilter::of(TimelineScope::Project(1))
            })
            .expect("the equal-instant until bound filters stored rows");

        assert_eq!(
            rows.len(),
            1,
            "an until bound naming the row's own instant must include it"
        );
        assert_eq!(rows[0].entity_id.as_deref(), Some("kan-t9"));
    }

    #[test]
    fn a_since_bound_equal_to_a_row_instant_includes_the_row() {
        let database = migrated_database();
        let recorded_at = append_and_record(
            &database,
            ticket_event(
                1,
                TimelineEventKind::Transition,
                "kan-t9",
                json!({ "to": "in_progress" }),
            ),
        );

        let (since, _) = validate_timeline_time_window(Some(&recorded_at), None)
            .expect("a stored instant is a valid since bound");

        let rows = database
            .query_timeline(&TimelineFilter {
                since,
                ..TimelineFilter::of(TimelineScope::Project(1))
            })
            .expect("the equal-instant since bound filters stored rows");

        assert_eq!(
            rows.len(),
            1,
            "a since bound naming the row's own instant must include it"
        );
        assert_eq!(rows[0].entity_id.as_deref(), Some("kan-t9"));
    }

    #[test]
    fn sub_millisecond_bounds_settle_onto_the_stored_millisecond() {
        let database = migrated_database();
        let recorded_at = append_and_record(
            &database,
            ticket_event(1, TimelineEventKind::Run, "kan-t9", json!({ "attempt": 1 })),
        );
        let finer = finer_than_the_stored_millisecond(&recorded_at);

        let (since, _) = validate_timeline_time_window(Some(&finer), None)
            .expect("a sub-millisecond bound is valid RFC 3339");
        let (_, until) = validate_timeline_time_window(None, Some(&finer))
            .expect("a sub-millisecond bound is valid RFC 3339");

        let after_the_row = database
            .query_timeline(&TimelineFilter {
                since,
                ..TimelineFilter::of(TimelineScope::Project(1))
            })
            .expect("the aligned since bound filters stored rows");
        let through_the_row = database
            .query_timeline(&TimelineFilter {
                until,
                ..TimelineFilter::of(TimelineScope::Project(1))
            })
            .expect("the aligned until bound filters stored rows");

        assert!(
            after_the_row.is_empty(),
            "a since inside the row's millisecond excludes the earlier row"
        );
        assert_eq!(
            through_the_row.len(),
            1,
            "an until inside the row's millisecond still includes the row"
        );
    }

    #[test]
    fn offset_bounds_settle_onto_stored_utc_rows() {
        let database = migrated_database();
        append(
            &database,
            ticket_event(1, TimelineEventKind::Run, "kan-t9", json!({ "attempt": 1 })),
        );
        append(
            &database,
            ticket_event(
                1,
                TimelineEventKind::Comment,
                "kan-t10",
                json!({ "text": "noted" }),
            ),
        );
        let rows = database
            .query_timeline(&TimelineFilter::of(TimelineScope::Project(1)))
            .expect("appended rows are readable");
        let earliest = rows
            .first()
            .expect("the first append landed")
            .recorded_at
            .clone();
        let latest = rows
            .last()
            .expect("the second append landed")
            .recorded_at
            .clone();

        let (since, until) = validate_timeline_time_window(
            Some(&at_offset(&earliest, 1)),
            Some(&at_offset(&latest, -5)),
        )
        .expect("offset bounds name stored UTC instants");

        let rows = database
            .query_timeline(&TimelineFilter {
                since,
                until,
                ..TimelineFilter::of(TimelineScope::Project(1))
            })
            .expect("normalised offset bounds filter stored rows");

        assert_eq!(
            rows.len(),
            2,
            "offset bounds naming the outer row instants include every row"
        );
    }

    #[test]
    fn a_window_beyond_every_row_is_empty() {
        let database = migrated_database();
        let recorded_at = append_and_record(
            &database,
            ticket_event(1, TimelineEventKind::Run, "kan-t9", json!({ "attempt": 1 })),
        );

        let (since, _) =
            validate_timeline_time_window(Some(&shifted_by_seconds(&recorded_at, 1)), None)
                .expect("a shifted bound is valid RFC 3339");
        let (_, until) =
            validate_timeline_time_window(None, Some(&shifted_by_seconds(&recorded_at, -1)))
                .expect("a shifted bound is valid RFC 3339");

        let later = database
            .query_timeline(&TimelineFilter {
                since,
                ..TimelineFilter::of(TimelineScope::Project(1))
            })
            .expect("the since bound filters stored rows");
        let earlier = database
            .query_timeline(&TimelineFilter {
                until,
                ..TimelineFilter::of(TimelineScope::Project(1))
            })
            .expect("the until bound filters stored rows");

        assert!(
            later.is_empty(),
            "no row records an instant after the only row"
        );
        assert!(
            earlier.is_empty(),
            "no row records an instant before the only row"
        );
    }

    #[test]
    fn malformed_bounds_are_refused_before_storage_access() {
        let error = validate_timeline_time_window(Some("not-a-timestamp"), None)
            .expect_err("malformed bounds never reach SQLite");

        assert_eq!(
            error,
            TimelineTimeError::MalformedBound {
                label: "since",
                value: "not-a-timestamp".to_owned(),
            }
        );
    }

    #[test]
    fn reversed_windows_are_refused_before_storage_access() {
        let error = validate_timeline_time_window(
            Some("2026-09-05T00:00:00Z"),
            Some("2026-09-04T00:00:00Z"),
        )
        .expect_err("reversed windows never reach SQLite");

        assert!(matches!(error, TimelineTimeError::ReversedWindow { .. }));
    }

    #[test]
    fn archived_entities_leave_timelines_intact_and_queryable() {
        let database = migrated_database();
        append(
            &database,
            ticket_event(
                1,
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
                ..TimelineFilter::of(TimelineScope::Project(1))
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
                 VALUES ('1', 'comment', '{\"text\":\"noted\"}')",
                [],
            )
            .expect("the legacy row lands");

        database
            .migrate(&AllowAllMigrations)
            .expect("the scope migration applies");

        let rows = database
            .query_timeline(&TimelineFilter::of(TimelineScope::Project(1)))
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
