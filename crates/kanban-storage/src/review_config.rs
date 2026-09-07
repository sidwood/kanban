//! SQLite staged review configurations (KAN-S10): one row per
//! Ticket, the stages as the JSON shape the wire carries, and the
//! version the application layer's optimistic checks guard (DR-EP-09).
//! The row change and its timeline append share one write.

use kanban_app::{ReviewConfigStore, TimelineEnvelope};
use kanban_domain::{
    ProfileName, ReviewConfiguration, ReviewSlot, ReviewSlotAssignment, ReviewStage,
    SlotRequirement, TicketId,
};
use kanban_dto::ApiError;
use rusqlite::params;
use serde_json::{Value, json};

use crate::db::{ConnectionHandle, Database, WriteSpan};
use crate::timeline::insert_event;

/// SQLite-backed review configurations.
pub struct SqliteReviewConfigStore {
    conn: ConnectionHandle,
}

impl SqliteReviewConfigStore {
    /// Share the connection the `database` owns.
    pub fn new(database: &Database) -> Self {
        Self {
            conn: database.connection_handle(),
        }
    }

    fn lock(&self) -> parking_lot::ReentrantMutexGuard<'_, rusqlite::Connection> {
        self.conn.lock()
    }
}

impl ReviewConfigStore for SqliteReviewConfigStore {
    fn insert(
        &self,
        ticket: TicketId,
        configuration: &ReviewConfiguration,
        envelope: &TimelineEnvelope,
    ) -> Result<(), ApiError> {
        let conn = self.lock();
        let span = WriteSpan::begin(&conn).map_err(internal)?;
        let outcome = span.execute(
            "INSERT INTO ticket_review_configurations (ticket_id, stages, version)
             VALUES (?1, ?2, ?3)",
            params![
                ticket.value() as i64,
                stages_json(configuration),
                configuration.version() as i64,
            ],
        );
        match outcome {
            Ok(_) => {}
            Err(error) if is_ticket_conflict(&error) => {
                let current = current_version(&span, ticket)?;
                return Err(ApiError::stale_version(0, current));
            }
            Err(error) => return Err(internal(error)),
        }
        insert_event(&span, envelope).map_err(internal)?;
        span.commit().map_err(internal)?;
        Ok(())
    }

    fn save(
        &self,
        ticket: TicketId,
        configuration: &ReviewConfiguration,
        envelope: &TimelineEnvelope,
    ) -> Result<(), ApiError> {
        let conn = self.lock();
        let span = WriteSpan::begin(&conn).map_err(internal)?;
        let moved_from = configuration.version() - 1;
        let changed = span
            .execute(
                "UPDATE ticket_review_configurations
                 SET stages = ?2, version = ?3
                 WHERE ticket_id = ?1 AND version = ?4",
                params![
                    ticket.value() as i64,
                    stages_json(configuration),
                    configuration.version() as i64,
                    moved_from as i64,
                ],
            )
            .map_err(internal)?;
        if changed != 1 {
            return Err(save_refused(&span, ticket, moved_from));
        }
        insert_event(&span, envelope).map_err(internal)?;
        span.commit().map_err(internal)?;
        Ok(())
    }

    fn find(&self, ticket: TicketId) -> Result<Option<ReviewConfiguration>, ApiError> {
        let conn = self.lock();
        match conn.query_row(
            "SELECT stages, version FROM ticket_review_configurations WHERE ticket_id = ?1",
            params![ticket.value() as i64],
            |row| {
                Ok(ReviewConfiguration::restore(
                    stages_of(&row.get::<_, String>(0)?).ok_or_else(|| {
                        rusqlite::Error::FromSqlConversionFailure(
                            0,
                            rusqlite::types::Type::Text,
                            Box::new(CorruptRow),
                        )
                    })?,
                    row.get::<_, i64>(1)? as u64,
                ))
            },
        ) {
            Ok(configuration) => Ok(Some(configuration)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(error) => Err(internal(error)),
        }
    }
}

/// The version the stored row stands at, for the stale-version
/// refusal a racing create reports.
fn current_version(span: &WriteSpan<'_>, ticket: TicketId) -> Result<u64, ApiError> {
    span.query_row(
        "SELECT version FROM ticket_review_configurations WHERE ticket_id = ?1",
        params![ticket.value() as i64],
        |row| row.get::<_, i64>(0),
    )
    .map(|version| version.unsigned_abs())
    .map_err(internal)
}

/// Why a guarded write was refused, read from the row's current
/// state.
fn save_refused(span: &WriteSpan<'_>, ticket: TicketId, attempted_from: u64) -> ApiError {
    match current_version(span, ticket) {
        Ok(current) => ApiError::stale_version(attempted_from, current),
        Err(_) => ApiError::not_found(&format!(
            "review configuration of ticket {}",
            ticket.value()
        )),
    }
}

/// The stages as one JSON array of the wire's shape: an object per
/// stage, a slot list per stage, and one tagged occupant per slot.
fn stages_json(configuration: &ReviewConfiguration) -> String {
    json!(
        configuration
            .stages()
            .iter()
            .map(|stage| json!({
                "slots": stage
                    .slots()
                    .iter()
                    .map(|slot| json!({
                        "occupant": match slot.assignment() {
                            ReviewSlotAssignment::Human => {
                                json!({ "kind": "human" })
                            }
                            ReviewSlotAssignment::Profile(name) => {
                                json!({ "kind": "profile", "name": name.as_str() })
                            }
                        },
                        "requirement": slot.requirement().wire_name(),
                    }))
                    .collect::<Vec<_>>(),
            }))
            .collect::<Vec<_>>()
    )
    .to_string()
}

/// The stages one stored JSON array names, or `None` outside the
/// vocabularies.
fn stages_of(text: &str) -> Option<Vec<ReviewStage>> {
    let parsed: Value = serde_json::from_str(text).ok()?;
    parsed
        .as_array()?
        .iter()
        .map(|stage| {
            let slots = stage
                .get("slots")?
                .as_array()?
                .iter()
                .map(|slot| {
                    let requirement = SlotRequirement::parse(slot.get("requirement")?.as_str()?)?;
                    let occupant = slot.get("occupant")?;
                    match occupant.get("kind")?.as_str()? {
                        "human" => Some(ReviewSlot::human(requirement)),
                        "profile" => Some(ReviewSlot::profile(
                            ProfileName::new(occupant.get("name")?.as_str()?).ok()?,
                            requirement,
                        )),
                        _ => None,
                    }
                })
                .collect::<Option<Vec<_>>>()?;
            Some(ReviewStage::restore(slots))
        })
        .collect()
}

/// Whether `error` is the row-level refusal of a second
/// configuration for one Ticket.
fn is_ticket_conflict(error: &rusqlite::Error) -> bool {
    error
        .to_string()
        .contains("UNIQUE constraint failed: ticket_review_configurations.ticket_id")
}

/// Report a SQLite failure the caller cannot act on.
fn internal(error: impl ToString) -> ApiError {
    ApiError::internal(&error.to_string())
}

/// A stored configuration row failed domain validation.
#[derive(Debug)]
struct CorruptRow;

impl std::fmt::Display for CorruptRow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "a stored review configuration row failed validation: stages"
        )
    }
}

impl std::error::Error for CorruptRow {}

#[cfg(test)]
mod review_config_rows {
    use kanban_domain::{
        ProfileName, ReviewConfiguration, ReviewSlot, ReviewStage, SlotRequirement, TicketId,
    };

    use super::ReviewConfigStore as _;
    use super::SqliteReviewConfigStore;
    use crate::migrations::AllowAllMigrations;
    use crate::test_support::scratch_database;
    use kanban_app::TimelineEnvelope;
    use kanban_dto::{TimelineEntityKind, TimelineEntityRef, TimelineEventKind};
    use serde_json::json;

    fn named(raw: &str) -> ProfileName {
        ProfileName::new(raw).expect("a non-blank name is accepted")
    }

    /// One varied configuration: two stages, the first holding a
    /// required profile slot and an optional human slot in parallel.
    fn configuration(version: u64) -> ReviewConfiguration {
        ReviewConfiguration::restore(
            vec![
                ReviewStage::restore(vec![
                    ReviewSlot::profile(named("outsider"), SlotRequirement::Required),
                    ReviewSlot::human(SlotRequirement::Optional),
                ]),
                ReviewStage::restore(vec![ReviewSlot::profile(
                    named("same-harness"),
                    SlotRequirement::Required,
                )]),
            ],
            version,
        )
    }

    fn envelope(ticket: u64) -> TimelineEnvelope {
        TimelineEnvelope::project(
            1,
            TimelineEventKind::Transition,
            Some(TimelineEntityRef {
                kind: TimelineEntityKind::Ticket,
                id: ticket.to_string(),
            }),
            json!({ "action": "review_configured", "id": ticket }),
        )
    }

    fn seeded() -> (tempfile::TempDir, crate::Database) {
        let (dir, mut database) = scratch_database();
        database
            .migrate(&AllowAllMigrations)
            .expect("the schema applies");
        database
            .connection()
            .execute(
                "INSERT INTO projects
                     (code, name, repository, seed_workspace, default_branch,
                      herdr_workspace, herdr_session, archived, version)
                 VALUES ('CORE', 'Control plane', '/repositories/kanban',
                         '/workspaces/kanban.seed', 'main', 'kanban.seed',
                         'kanban-main', 0, 1)",
                [],
            )
            .expect("the fixture Project lands");
        database
            .connection()
            .execute(
                "INSERT INTO tickets
                     (project_id, number, kind, priority, state, title, criteria,
                      subtype, mode, completion, version)
                 VALUES (1, 1, 'task', 'normal', 'draft', 'One slice', '[]',
                         'operational', 'human', '[\"done\"]', 1)",
                [],
            )
            .expect("the fixture Ticket lands");
        (dir, database)
    }

    #[test]
    fn a_row_round_trips_every_stage_and_survives_reopen() {
        let (dir, database) = seeded();
        let store = SqliteReviewConfigStore::new(&database);
        let ticket = TicketId::new(1);
        store
            .insert(ticket, &configuration(1), &envelope(1))
            .expect("the configuration lands");
        drop(database);

        let database =
            crate::Database::open(&dir.path().join("kanban.sqlite")).expect("the database reopens");
        let restored = SqliteReviewConfigStore::new(&database)
            .find(ticket)
            .expect("the reload serves")
            .expect("the configuration is durable");

        assert_eq!(restored, configuration(1));
        assert_eq!(restored.stages().len(), 2);
        assert_eq!(
            restored.stages()[0].slots()[0]
                .profile_name()
                .map(|name| name.as_str()),
            Some("outsider")
        );
        assert_eq!(
            restored.stages()[0].slots()[0].requirement(),
            SlotRequirement::Required
        );
        assert!(restored.stages()[0].slots()[1].is_human());
        assert_eq!(
            restored.stages()[1].slots()[0]
                .profile_name()
                .map(|name| name.as_str()),
            Some("same-harness")
        );
    }

    #[test]
    fn inserting_a_second_row_for_one_ticket_is_refused_as_stale() {
        let (_dir, database) = seeded();
        let store = SqliteReviewConfigStore::new(&database);
        let ticket = TicketId::new(1);
        store
            .insert(ticket, &configuration(1), &envelope(1))
            .expect("the first configuration lands");

        let error = store
            .insert(ticket, &configuration(1), &envelope(1))
            .expect_err("one Ticket holds one stored configuration");

        assert_eq!(error.code, kanban_dto::ErrorCode::StaleVersion);
        assert_eq!(error.current_version, Some(1));
    }

    #[test]
    fn replacing_is_guarded_by_the_version_the_row_stands_at() {
        let (_dir, database) = seeded();
        let store = SqliteReviewConfigStore::new(&database);
        let ticket = TicketId::new(1);
        store
            .insert(ticket, &configuration(1), &envelope(1))
            .expect("the first configuration lands");

        let stale = store
            .save(ticket, &configuration(3), &envelope(1))
            .expect_err("a write from the wrong version is refused");
        assert_eq!(stale.code, kanban_dto::ErrorCode::StaleVersion);
        assert_eq!(stale.current_version, Some(1));

        store
            .save(ticket, &configuration(2), &envelope(1))
            .expect("the replacement lands from the standing version");
        assert_eq!(
            store
                .find(ticket)
                .expect("the find serves")
                .unwrap()
                .version(),
            2
        );

        let missing = store
            .save(TicketId::new(9), &configuration(2), &envelope(9))
            .expect_err("a row that stands nowhere is not found");
        assert_eq!(missing.code, kanban_dto::ErrorCode::NotFound);
    }

    #[test]
    fn each_write_lands_its_timeline_row_in_the_same_commit() {
        let (_dir, database) = seeded();
        let store = SqliteReviewConfigStore::new(&database);
        let ticket = TicketId::new(1);
        store
            .insert(ticket, &configuration(1), &envelope(1))
            .expect("the configuration lands");
        store
            .save(ticket, &configuration(2), &envelope(1))
            .expect("the replacement lands");

        let recorded: Vec<_> = database
            .connection()
            .prepare(
                "SELECT kind, entity_kind, entity_id, detail FROM timeline_events
                 WHERE scope = 'project' AND project_id = '1'
                 ORDER BY id",
            )
            .expect("the timeline is readable")
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    serde_json::from_str::<serde_json::Value>(&row.get::<_, String>(3)?)
                        .expect("the detail is JSON"),
                ))
            })
            .expect("the timeline rows serve")
            .collect::<Result<Vec<_>, _>>()
            .expect("the timeline rows decode");
        assert_eq!(
            recorded,
            vec![
                (
                    "transition".to_owned(),
                    "ticket".to_owned(),
                    "1".to_owned(),
                    json!({
                        "action": "review_configured", "id": 1,
                    })
                ),
                (
                    "transition".to_owned(),
                    "ticket".to_owned(),
                    "1".to_owned(),
                    json!({
                        "action": "review_configured", "id": 1,
                    })
                ),
            ],
            "every write appends its own timeline row"
        );
    }
}
