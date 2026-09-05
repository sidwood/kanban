//! The SQLite implementation of the Ticket storage port: rows in
//! `tickets` carrying the Project, the minted number, the kind whose
//! schema the Ticket holds, the priority, the lifecycle state, and
//! the kind-specific fields, with the application's timeline envelope
//! landing unchanged in the same transaction as every change. Creating
//! a Ticket persists the Project counter its number minted in the same
//! write. The schema-level CHECK keeps each kind to exactly its own
//! fields and the trigger keeps Tickets never deleted; every stored
//! value passed domain validation on the way in, so a row that fails
//! to rehydrate is corruption the caller must hear about.

use kanban_app::{TicketStore, TimelineEnvelope};
use kanban_domain::{
    AcceptanceCriterion, BugTicket, ImplementationTicket, Priority, Project, ProjectId, SpecId,
    SpecNumber, TaskTicket, Ticket, TicketBody, TicketId, TicketNumber, TicketState, UserStoryRef,
};
use kanban_dto::ApiError;
use rusqlite::params;
use serde::{Deserialize, Serialize};

use crate::db::{ConnectionHandle, Database, WriteSpan};
use crate::timeline::insert_event;

/// Every stored column of one Ticket row, in select order.
const TICKET_COLUMNS: &str =
    "id, project_id, number, kind, priority, state, spec_id, title, slice, criteria, version";

/// The Ticket port over the authoritative database.
pub struct SqliteTicketStore {
    conn: ConnectionHandle,
}

impl SqliteTicketStore {
    /// Share the connection the `database` owns.
    pub fn new(database: &Database) -> Self {
        Self {
            conn: database.connection_handle(),
        }
    }

    /// Lock the shared connection.
    fn lock(&self) -> parking_lot::ReentrantMutexGuard<'_, rusqlite::Connection> {
        self.conn.lock()
    }
}

impl TicketStore for SqliteTicketStore {
    fn create(
        &self,
        project: &Project,
        number: TicketNumber,
        priority: Priority,
        body: &TicketBody,
        envelope: &dyn Fn(TicketId) -> TimelineEnvelope,
    ) -> Result<Ticket, ApiError> {
        let conn = self.lock();
        let span = WriteSpan::begin(&conn).map_err(internal)?;
        // The minted number and the Project row move together: a
        // stale writer can never rewind a minted counter.
        let preceding_version = project.version() - 1;
        let changed = span
            .execute(
                "UPDATE projects
                 SET ticket_counter = ?2,
                     version = ?3
                 WHERE id = ?1 AND version = ?4",
                params![
                    project.id().value() as i64,
                    number.value() as i64,
                    project.version() as i64,
                    preceding_version as i64,
                ],
            )
            .map_err(internal)?;
        if changed != 1 {
            return Err(project_write_refused(
                &span,
                project.id(),
                preceding_version,
            ));
        }
        let stored = StoredTicket::of(priority, body);
        span.execute(
            "INSERT INTO tickets
                 (project_id, number, kind, priority, state, spec_id, title, slice,
                  criteria, version)
             VALUES (?1, ?2, ?3, ?4, 'draft', ?5, ?6, ?7, ?8, 1)",
            params![
                project.id().value() as i64,
                number.value() as i64,
                stored.kind,
                stored.priority,
                stored.spec_id,
                stored.title,
                stored.slice,
                stored.criteria,
            ],
        )
        .map_err(internal)?;
        let id = TicketId::new(
            span.last_insert_rowid()
                .try_into()
                .map_err(|_| ApiError::internal("the Ticket identity overflowed"))?,
        );
        append_timeline(&span, &envelope(id))?;
        span.commit().map_err(internal)?;
        Ok(Ticket::new(
            id,
            project.id(),
            number,
            priority,
            body.clone(),
        ))
    }

    fn find(&self, id: TicketId) -> Result<Option<Ticket>, ApiError> {
        let conn = self.lock();
        let row = conn.query_row(
            &format!("SELECT {TICKET_COLUMNS} FROM tickets WHERE id = ?1"),
            params![id.value() as i64],
            load_ticket_row,
        );
        match row {
            Ok(loaded) => loaded.rehydrate().map(Some).map_err(internal),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(error) => Err(internal(error)),
        }
    }

    fn list(&self, project: ProjectId) -> Result<Vec<Ticket>, ApiError> {
        let conn = self.lock();
        let mut statement = conn
            .prepare(&format!(
                "SELECT {TICKET_COLUMNS} FROM tickets WHERE project_id = ?1 ORDER BY id"
            ))
            .map_err(internal)?;
        let rows = statement
            .query_map(params![project.value() as i64], load_ticket_row)
            .map_err(internal)?;
        let mut tickets = Vec::new();
        for row in rows {
            let loaded = row.map_err(internal)?;
            tickets.push(loaded.rehydrate().map_err(internal)?);
        }
        Ok(tickets)
    }
}

/// One decoded `tickets` row before its body is assembled.
struct LoadedTicket {
    id: u64,
    project: u64,
    number: u64,
    kind: String,
    priority: String,
    state: String,
    spec_id: Option<i64>,
    title: Option<String>,
    slice: Option<String>,
    criteria: String,
    version: u64,
}

/// Decode one `tickets` row.
fn load_ticket_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<LoadedTicket> {
    Ok(LoadedTicket {
        id: row.get::<_, i64>(0)?.unsigned_abs(),
        project: row.get::<_, i64>(1)?.unsigned_abs(),
        number: row.get::<_, i64>(2)?.unsigned_abs(),
        kind: row.get::<_, String>(3)?,
        priority: row.get::<_, String>(4)?,
        state: row.get::<_, String>(5)?,
        spec_id: row.get::<_, Option<i64>>(6)?,
        title: row.get::<_, Option<String>>(7)?,
        slice: row.get::<_, Option<String>>(8)?,
        criteria: row.get::<_, String>(9)?,
        version: row.get::<_, i64>(10)?.unsigned_abs(),
    })
}

impl LoadedTicket {
    /// Assemble the aggregate. Every stored value passed validation on
    /// the way in, so a failure here is corruption the caller must
    /// hear about, not silently accept.
    fn rehydrate(&self) -> Result<Ticket, rusqlite::Error> {
        let body = match self.kind.as_str() {
            "implementation" => TicketBody::Implementation(ImplementationTicket::restore(
                SpecId::new(self.spec_id.ok_or_else(corrupt)?.unsigned_abs()),
                self.slice.clone().ok_or_else(corrupt)?,
                decode_criteria(&self.criteria)?,
            )),
            "bug" => TicketBody::Bug(BugTicket::restore(
                self.title.clone().ok_or_else(corrupt)?,
                self.spec_id.map(|spec| SpecId::new(spec.unsigned_abs())),
            )),
            "task" => TicketBody::Task(TaskTicket::restore(
                self.title.clone().ok_or_else(corrupt)?,
                self.spec_id.map(|spec| SpecId::new(spec.unsigned_abs())),
            )),
            _ => return Err(corrupt()),
        };
        Ok(Ticket::restore(
            TicketId::new(self.id),
            ProjectId::new(self.project),
            TicketNumber::new(self.number).map_err(|_| corrupt())?,
            Priority::parse(&self.priority).ok_or_else(corrupt)?,
            TicketState::parse(&self.state).ok_or_else(corrupt)?,
            body,
            self.version,
        ))
    }
}

/// The stored form of one Ticket's columns, ready to insert.
struct StoredTicket<'a> {
    kind: &'a str,
    priority: &'a str,
    spec_id: Option<i64>,
    title: Option<&'a str>,
    slice: Option<&'a str>,
    criteria: String,
}

impl<'a> StoredTicket<'a> {
    /// Flatten one validated body into its stored columns.
    fn of(priority: Priority, body: &'a TicketBody) -> Self {
        match body {
            TicketBody::Implementation(implementation) => Self {
                kind: "implementation",
                priority: priority.wire_name(),
                spec_id: Some(implementation.spec().value() as i64),
                title: None,
                slice: Some(implementation.slice()),
                criteria: encode_criteria(implementation.criteria()),
            },
            TicketBody::Bug(bug) => Self {
                kind: "bug",
                priority: priority.wire_name(),
                spec_id: bug.spec().map(|spec| spec.value() as i64),
                title: Some(bug.title()),
                slice: None,
                criteria: "[]".to_owned(),
            },
            TicketBody::Task(task) => Self {
                kind: "task",
                priority: priority.wire_name(),
                spec_id: task.spec().map(|spec| spec.value() as i64),
                title: Some(task.title()),
                slice: None,
                criteria: "[]".to_owned(),
            },
        }
    }
}

/// One criterion's stored form: the outcome and the stories it claims
/// as ordinal pairs, so rehydration needs no Project code.
#[derive(Serialize, Deserialize)]
struct StoredCriterion {
    outcome: String,
    stories: Vec<StoredStory>,
}

/// One story link's stored form: the Spec's minted number and the
/// story's ordinal.
#[derive(Serialize, Deserialize)]
struct StoredStory {
    spec: u64,
    story: u64,
}

/// Encode the criteria the domain validated.
fn encode_criteria(criteria: &[AcceptanceCriterion]) -> String {
    let stored: Vec<StoredCriterion> = criteria
        .iter()
        .map(|criterion| StoredCriterion {
            outcome: criterion.outcome().to_owned(),
            stories: criterion
                .stories()
                .iter()
                .map(|story| StoredStory {
                    spec: story.spec().value(),
                    story: story.story(),
                })
                .collect(),
        })
        .collect();
    serde_json::to_string(&stored).expect("the criteria serialise")
}

/// Decode stored criteria back into the domain's rule-valid form.
fn decode_criteria(stored: &str) -> Result<Vec<AcceptanceCriterion>, rusqlite::Error> {
    let rows: Vec<StoredCriterion> = serde_json::from_str(stored).map_err(|_| corrupt())?;
    let mut criteria = Vec::with_capacity(rows.len());
    for row in rows {
        let mut stories = Vec::with_capacity(row.stories.len());
        for story in row.stories {
            let spec = SpecNumber::new(story.spec).map_err(|_| corrupt())?;
            stories.push(UserStoryRef::new(spec, story.story).map_err(|_| corrupt())?);
        }
        criteria.push(AcceptanceCriterion::new(row.outcome, stories).map_err(|_| corrupt())?);
    }
    Ok(criteria)
}

/// Why a guarded Project write was refused, read from the row's
/// current state.
fn project_write_refused(
    conn: &rusqlite::Connection,
    id: ProjectId,
    attempted_from: u64,
) -> ApiError {
    match conn.query_row(
        "SELECT version FROM projects WHERE id = ?1",
        params![id.value() as i64],
        |row| row.get::<_, i64>(0),
    ) {
        Ok(current) => ApiError::stale_version(attempted_from, current.unsigned_abs()),
        Err(rusqlite::Error::QueryReturnedNoRows) => ApiError::not_found(&format!("project {id}")),
        Err(error) => internal(error),
    }
}

/// Report a SQLite failure the caller cannot act on.
fn internal(error: rusqlite::Error) -> ApiError {
    ApiError::internal(&error.to_string())
}

/// A stored Ticket row failed domain validation.
#[derive(Debug)]
struct CorruptRow;

impl std::fmt::Display for CorruptRow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "a stored Ticket row failed validation")
    }
}

impl std::error::Error for CorruptRow {}

/// The SQLite failure a corrupt row reports.
fn corrupt() -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(CorruptRow))
}

/// Insert the application's envelope, unchanged, on the same
/// transaction as the row it records.
fn append_timeline(
    conn: &rusqlite::Connection,
    envelope: &TimelineEnvelope,
) -> Result<(), ApiError> {
    insert_event(conn, envelope).map_err(|error| ApiError::internal(&error.to_string()))
}

#[cfg(test)]
mod tests {
    use kanban_app::{ProjectStore, SpecStore, TicketStore, TimelineEnvelope};
    use kanban_domain::{
        NumberKind, Priority, Project, ProjectCounters, ProjectId, ProjectRegistration,
        ProjectState, SpecContent, SpecId, SpecNumber, TicketBody, TicketId, TicketNumber,
    };
    use kanban_dto::{
        ErrorCode, TimelineEntityKind, TimelineEntityRef, TimelineEventKind, TimelineScope,
    };
    use serde_json::json;

    use super::SqliteTicketStore;
    use crate::db::Database;
    use crate::migrations::AllowAllMigrations;
    use crate::projects::SqliteProjectStore;
    use crate::spec::SqliteSpecStore;
    use crate::test_support::scratch_database;
    use crate::timeline::TimelineFilter;

    fn story(spec: u64, ordinal: u64) -> kanban_domain::UserStoryRef {
        kanban_domain::UserStoryRef::new(
            SpecNumber::new(spec).expect("the fixture number is positive"),
            ordinal,
        )
        .expect("the fixture ordinal is positive")
    }

    /// An Implementation body delivering the seeded Spec's behaviour.
    fn implementation(spec: SpecId) -> TicketBody {
        TicketBody::implementation(
            Some(spec),
            SpecNumber::new(1).expect("the fixture number is positive"),
            "Spec authoring creates content versions end to end",
            vec![
                kanban_domain::AcceptanceCriterion::new(
                    "Specs mint unique numbers.",
                    vec![story(1, 1)],
                )
                .expect("the fixture criterion links"),
            ],
        )
        .expect("the fixture body validates")
    }

    fn store() -> (tempfile::TempDir, Database, SqliteTicketStore) {
        let (dir, mut database) = scratch_database();
        database
            .migrate(&AllowAllMigrations)
            .expect("the migrations apply");
        let store = SqliteTicketStore::new(&database);
        (dir, database, store)
    }

    fn registration() -> ProjectRegistration {
        ProjectRegistration::new(
            "CORE",
            "Control plane",
            "/repositories/kanban",
            "/workspaces/kanban.seed",
            "main",
            "kanban.seed",
            Some("kanban-main"),
            None,
        )
        .expect("the fixture registration validates")
    }

    /// Seed the Project and Spec rows the tickets write against.
    fn seeded_project_and_spec(database: &Database) -> (Project, SpecId) {
        let projects = SqliteProjectStore::new(database);
        let created = projects
            .create(&registration(), &|id| {
                TimelineEnvelope::project(
                    id.value(),
                    TimelineEventKind::Transition,
                    Some(TimelineEntityRef {
                        kind: TimelineEntityKind::Project,
                        id: id.value().to_string(),
                    }),
                    json!({ "action": "registered", "id": id.value() }),
                )
            })
            .expect("the fixture Project lands");
        let mut project = Project::restore(
            created.id(),
            created.registration().clone(),
            ProjectState::Active,
            ProjectCounters::restore(0, 0, 0),
            1,
        );
        let specs = SqliteSpecStore::new(database);
        let number =
            SpecNumber::new(project.mint(NumberKind::Spec)).expect("a minted number is positive");
        let spec = specs
            .create(&project, number, &spec_content(), &|id| {
                TimelineEnvelope::project(
                    1,
                    TimelineEventKind::Transition,
                    Some(TimelineEntityRef {
                        kind: TimelineEntityKind::Spec,
                        id: id.value().to_string(),
                    }),
                    json!({ "action": "created", "id": id.value(), "number": number.value() }),
                )
            })
            .expect("the fixture Spec lands");
        project = projects
            .find(project.id())
            .expect("the reload serves")
            .expect("the Project exists");
        (project, spec.id())
    }

    fn spec_content() -> SpecContent {
        SpecContent::new(
            "Registration",
            "Versioned Plan graphs of Specs",
            "Planning must survive change.",
            "Immutable approved versions.",
            "- CORE-S1-US1: As an operator, I want unique numbers.",
            "Supersession is explicit.",
            "Domain tests prove immutability.",
            "The Ticket graph proposal.",
            "None",
        )
        .expect("the fixture content validates")
    }

    /// The envelope the application layer builds for one Ticket
    /// transition, on the seeded Project's timeline.
    fn transition(ticket: TicketId, action: &str, facts: serde_json::Value) -> TimelineEnvelope {
        let mut detail = facts;
        let object = detail.as_object_mut().expect("the facts are an object");
        object.insert("action".to_owned(), serde_json::Value::from(action));
        object.insert("id".to_owned(), serde_json::Value::from(ticket.value()));
        TimelineEnvelope::project(
            1,
            TimelineEventKind::Transition,
            Some(TimelineEntityRef {
                kind: TimelineEntityKind::Ticket,
                id: ticket.value().to_string(),
            }),
            detail,
        )
    }

    /// Create one Ticket through the port, minting its number on the
    /// stored Project aggregate, and return the stored Ticket.
    fn created(
        store: &SqliteTicketStore,
        database: &Database,
        priority: Priority,
        body: &TicketBody,
    ) -> kanban_domain::Ticket {
        let mut project = SqliteProjectStore::new(database)
            .find(ProjectId::new(1))
            .expect("the reload serves")
            .expect("the Project exists");
        let number = TicketNumber::new(project.mint(NumberKind::Ticket))
            .expect("a minted number is positive");
        store
            .create(&project, number, priority, body, &|id| {
                transition(
                    id,
                    "created",
                    json!({ "project_id": 1, "number": number.value(), "kind": "bug" }),
                )
            })
            .expect("the Ticket lands")
    }

    /// Every ticket-scoped timeline row's detail, in landing order.
    fn ticket_timeline(database: &Database) -> Vec<serde_json::Value> {
        let conn = database.connection();
        let mut statement = conn
            .prepare(
                "SELECT detail FROM timeline_events
                 WHERE scope = 'project' AND entity_kind = 'ticket' ORDER BY id",
            )
            .expect("the timeline is readable");
        statement
            .query_map([], |row| {
                let detail: String = row.get(0)?;
                Ok(serde_json::from_str(&detail).expect("stored detail is JSON"))
            })
            .expect("the query runs")
            .collect::<Result<Vec<_>, _>>()
            .expect("the rows decode")
    }

    #[test]
    fn creating_lands_the_row_the_counter_and_the_timeline_append() {
        let (_dir, database, store) = store();
        let (_project, _spec) = seeded_project_and_spec(&database);

        let ticket = created(
            &store,
            &database,
            Priority::Urgent,
            &TicketBody::bug("Landing drops the integration branch", None)
                .expect("the fixture body validates"),
        );

        assert_eq!(ticket.id().value(), 1);
        assert_eq!(ticket.state(), kanban_domain::TicketState::Draft);
        let stored: (i64, i64, i64) = database
            .connection()
            .query_row(
                "SELECT (SELECT ticket_counter FROM projects WHERE id = 1),
                        (SELECT version FROM projects WHERE id = 1),
                        (SELECT COUNT(*) FROM tickets WHERE project_id = 1)",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("the rows are readable");
        assert_eq!(
            stored,
            (1, 3, 1),
            "the minted number, the Project's version move, and the Ticket row land together"
        );
        assert_eq!(
            ticket_timeline(&database),
            vec![json!({
                "action": "created",
                "id": 1,
                "project_id": 1,
                "number": 1,
                "kind": "bug",
            })],
            "the envelope reaches the Project's own timeline unchanged"
        );
    }

    #[test]
    fn the_created_implementation_round_trips_with_its_criteria() {
        let (_dir, database, store) = store();
        let (_project, spec) = seeded_project_and_spec(&database);

        let ticket = created(&store, &database, Priority::High, &implementation(spec));

        let found = store
            .find(ticket.id())
            .expect("the find serves")
            .expect("the Ticket exists");

        assert_eq!(found, ticket);
        assert_eq!(found.kind(), kanban_domain::TicketKind::Implementation);
        assert_eq!(found.spec(), Some(spec));
        assert_eq!(
            found.slice(),
            Some("Spec authoring creates content versions end to end")
        );
        assert_eq!(found.criteria().len(), 1);
        assert_eq!(found.criteria()[0].stories(), [story(1, 1)].as_slice());
        assert_eq!(found.version(), 1);
    }

    #[test]
    fn bug_and_task_bodies_round_trip_with_their_attachments() {
        let (_dir, database, store) = store();
        let (_project, spec) = seeded_project_and_spec(&database);

        let bug = created(
            &store,
            &database,
            Priority::Low,
            &TicketBody::bug("Landing drops the integration branch", None)
                .expect("the fixture body validates"),
        );
        let task = created(
            &store,
            &database,
            Priority::Normal,
            &TicketBody::task("Archive the old register", Some(spec))
                .expect("the fixture body validates"),
        );

        let found_bug = store
            .find(bug.id())
            .expect("the find serves")
            .expect("the Ticket exists");
        assert_eq!(
            found_bug.title(),
            Some("Landing drops the integration branch")
        );
        assert_eq!(found_bug.spec(), None, "a Bug may stand alone");

        let found_task = store
            .find(task.id())
            .expect("the find serves")
            .expect("the Ticket exists");
        assert_eq!(found_task.title(), Some("Archive the old register"));
        assert_eq!(
            found_task.spec(),
            Some(spec),
            "a Task may attach to one Spec"
        );
    }

    #[test]
    fn creating_two_tickets_mints_unique_numbers() {
        let (_dir, database, store) = store();
        let (_project, spec) = seeded_project_and_spec(&database);

        let first = created(&store, &database, Priority::Normal, &implementation(spec));
        // The helper reloads the Project row each create guards on,
        // so the second mint follows the first.
        let second = created(
            &store,
            &database,
            Priority::Normal,
            &TicketBody::task("Archive the old register", None)
                .expect("the fixture body validates"),
        );

        assert_ne!(first.id(), second.id());
        assert_eq!(
            (first.number().value(), second.number().value()),
            (1, 2),
            "minted numbers never collide"
        );
    }

    #[test]
    fn a_stale_create_is_refused_without_a_row_or_a_timeline_append() {
        let (_dir, database, store) = store();
        let (project, spec) = seeded_project_and_spec(&database);
        // A second Ticket moves the Project row past the aggregate the
        // stale create guards on.
        let _ = created(
            &store,
            &database,
            Priority::Normal,
            &TicketBody::bug("Landing drops the integration branch", None)
                .expect("the fixture body validates"),
        );
        let timeline_before = ticket_timeline(&database).len();

        let mut stale = project.clone();
        let number =
            TicketNumber::new(stale.mint(NumberKind::Ticket)).expect("a minted number is positive");
        let error = store
            .create(
                &stale,
                number,
                Priority::Normal,
                &implementation(spec),
                &|id| transition(id, "created", json!({})),
            )
            .expect_err("the stale create is refused");

        assert_eq!(error.code, ErrorCode::StaleVersion);
        let rows: i64 = database
            .connection()
            .query_row("SELECT COUNT(*) FROM tickets", [], |row| row.get(0))
            .expect("the rows are readable");
        assert_eq!(rows, 1, "a stale create must not land a row");
        assert_eq!(
            ticket_timeline(&database).len(),
            timeline_before,
            "a stale create must not append a timeline row"
        );
    }

    #[test]
    fn listing_covers_every_ticket_of_one_project_in_id_order() {
        let (_dir, database, store) = store();
        let (_project, spec) = seeded_project_and_spec(&database);
        created(&store, &database, Priority::Normal, &implementation(spec));
        created(
            &store,
            &database,
            Priority::Normal,
            &TicketBody::bug("Landing drops the integration branch", None)
                .expect("the fixture body validates"),
        );

        let listed = store.list(ProjectId::new(1)).expect("the list serves");

        let numbers: Vec<_> = listed
            .iter()
            .map(|ticket| ticket.number().value())
            .collect();
        assert_eq!(numbers, vec![1, 2]);
        assert!(
            store
                .list(ProjectId::new(9))
                .expect("the list serves")
                .is_empty(),
            "another Project's Tickets stay out"
        );
    }

    #[test]
    fn deleting_a_ticket_is_refused_by_the_schema() {
        let (_dir, database, store) = store();
        let (_project, _spec) = seeded_project_and_spec(&database);
        let ticket = created(
            &store,
            &database,
            Priority::Normal,
            &TicketBody::bug("Landing drops the integration branch", None)
                .expect("the fixture body validates"),
        );

        let outcome = store.lock().execute(
            "DELETE FROM tickets WHERE id = ?1",
            rusqlite::params![ticket.id().value() as i64],
        );

        let error = outcome.expect_err("the schema must refuse deletes");
        assert!(
            error.to_string().contains("never deleted"),
            "the refusal should say never deleted, got: {error}"
        );
    }

    #[test]
    fn the_schema_keeps_each_kind_to_its_own_fields() {
        let (_dir, database, store) = store();
        let (_project, spec) = seeded_project_and_spec(&database);
        created(&store, &database, Priority::Normal, &implementation(spec));

        let conn = database.connection();
        // A Bug shape carrying the Implementation's slice, an
        // Implementation without its Spec, and one without criteria:
        // each violates its kind's schema.
        for (sql, params) in [
            (
                "INSERT INTO tickets
                     (project_id, number, kind, priority, state, spec_id, title, slice,
                      criteria, version)
                 VALUES (1, 9, 'bug', 'normal', 'draft', NULL, 'A title', 'A slice', '[]', 1)",
                Vec::new(),
            ),
            (
                "INSERT INTO tickets
                     (project_id, number, kind, priority, state, spec_id, title, slice,
                      criteria, version)
                 VALUES (1, 9, 'implementation', 'normal', 'draft', NULL, NULL, 'A slice', '[]', 1)",
                Vec::new(),
            ),
            (
                "INSERT INTO tickets
                     (project_id, number, kind, priority, state, spec_id, title, slice,
                      criteria, version)
                 VALUES (1, 9, 'implementation', 'normal', 'draft', ?1, NULL, 'A slice', '[]', 1)",
                vec![spec.value() as i64],
            ),
        ] {
            let outcome = if params.is_empty() {
                conn.execute(sql, [])
            } else {
                conn.execute(sql, rusqlite::params_from_iter(params))
            };
            let error = outcome.expect_err("the kind's schema is closed");
            assert!(
                error.to_string().contains("CHECK constraint failed"),
                "`{sql}` should refuse, got: {error}"
            );
        }
        let stored: i64 = conn
            .query_row("SELECT COUNT(*) FROM tickets", [], |row| row.get(0))
            .expect("the rows are readable");
        assert_eq!(stored, 1, "every refused statement left the row intact");
    }

    #[test]
    fn ticket_history_decodes_from_the_projects_own_timeline() {
        let (_dir, database, store) = store();
        let (_project, _spec) = seeded_project_and_spec(&database);
        created(
            &store,
            &database,
            Priority::Normal,
            &TicketBody::task("Archive the old register", None)
                .expect("the fixture body validates"),
        );

        let rows = database
            .query_timeline(&TimelineFilter::of(TimelineScope::Project(1)))
            .expect("the Project timeline is readable");

        let ticket_rows: Vec<_> = rows
            .iter()
            .filter(|row| row.entity_kind.as_deref() == Some("ticket"))
            .collect();
        assert_eq!(ticket_rows.len(), 1, "creation");
        assert_eq!(
            TimelineEventKind::parse(&ticket_rows[0].kind),
            Some(TimelineEventKind::Transition),
            "`{}` must decode without migration repair",
            ticket_rows[0].kind
        );
        assert_eq!(
            ticket_rows[0].detail["action"],
            serde_json::json!("created")
        );
    }

    #[test]
    fn the_store_serves_through_a_shared_connection() {
        let (_dir, database, store) = store();
        let (project, _spec) = seeded_project_and_spec(&database);
        let boxed: Box<dyn TicketStore> = Box::new(store);
        let mut project = project;
        let number = TicketNumber::new(project.mint(NumberKind::Ticket))
            .expect("a minted number is positive");
        let body = TicketBody::bug("Landing drops the integration branch", None)
            .expect("the fixture body validates");

        let served = std::thread::spawn(move || {
            boxed
                .create(&project, number, Priority::Normal, &body, &|id| {
                    transition(id, "created", json!({}))
                })
                .map(|ticket| ticket.number().value())
        })
        .join()
        .expect("the serving thread finishes");

        assert_eq!(
            served.expect("the threaded creation lands"),
            1,
            "the port is Send + Sync over one connection"
        );
    }
}
