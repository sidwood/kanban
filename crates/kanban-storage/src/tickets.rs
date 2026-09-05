//! The SQLite implementation of the Ticket storage port: rows in
//! `tickets` carrying the Project, the minted number, the kind whose
//! schema the Ticket holds, the priority, the lifecycle state, and
//! the kind-specific fields — including a Bug's capture facts, its
//! qualification, and the vendor-neutral collections it carries —
//! with the application's timeline envelope landing unchanged in the
//! same transaction as every change. Creating a Ticket persists the
//! Project counter its number minted in the same write; saving an
//! applied Ticket moves its row under the version the aggregate moved
//! from. The schema-level CHECK and shape triggers keep each kind to
//! exactly its own fields and the trigger keeps Tickets never
//! deleted; every stored value passed domain validation on the way
//! in, so a row that fails to rehydrate is corruption the caller
//! must hear about.

use kanban_app::{TicketStore, TimelineEnvelope};
use kanban_domain::{
    AcceptanceCriterion, BugFacts, BugQualification, BugTicket, EvidenceId, ExternalReference,
    ImplementationTicket, OccurrenceSnapshot, Priority, Project, ProjectId, Severity, SpecId,
    SpecNumber, TaskTicket, Ticket, TicketBody, TicketId, TicketNumber, TicketState, UserStoryRef,
    VerificationStep,
};
use kanban_dto::ApiError;
use rusqlite::params;
use serde::{Deserialize, Serialize};

use crate::db::{ConnectionHandle, Database, WriteSpan};
use crate::timeline::insert_event;

/// Every stored column of one Ticket row, in select order.
const TICKET_COLUMNS: &str = "id, project_id, number, kind, priority, state, spec_id, title, \
                              slice, criteria, actual_behaviour, reporter_evidence, \
                              bug_qualification, bug_facts, version";

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
                  criteria, actual_behaviour, reporter_evidence, bug_qualification,
                  bug_facts, version)
             VALUES (?1, ?2, ?3, ?4, 'draft', ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, 1)",
            params![
                project.id().value() as i64,
                number.value() as i64,
                stored.kind,
                stored.priority,
                stored.spec_id,
                stored.title,
                stored.slice,
                stored.criteria,
                stored.actual_behaviour,
                stored.reporter_evidence,
                stored.bug_qualification,
                stored.bug_facts,
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

    fn save(&self, ticket: &Ticket, envelope: TimelineEnvelope) -> Result<(), ApiError> {
        let conn = self.lock();
        let span = WriteSpan::begin(&conn).map_err(internal)?;
        let stored = StoredTicket::of(ticket.priority(), ticket.body());
        let preceding_version = ticket.version() - 1;
        let changed = span
            .execute(
                "UPDATE tickets
                 SET priority = ?2, state = ?3, spec_id = ?4, title = ?5, slice = ?6,
                     criteria = ?7, actual_behaviour = ?8, reporter_evidence = ?9,
                     bug_qualification = ?10, bug_facts = ?11, version = ?12
                 WHERE id = ?1 AND version = ?13",
                params![
                    ticket.id().value() as i64,
                    stored.priority,
                    ticket.state().wire_name(),
                    stored.spec_id,
                    stored.title,
                    stored.slice,
                    stored.criteria,
                    stored.actual_behaviour,
                    stored.reporter_evidence,
                    stored.bug_qualification,
                    stored.bug_facts,
                    ticket.version() as i64,
                    preceding_version as i64,
                ],
            )
            .map_err(internal)?;
        if changed != 1 {
            return Err(ticket_write_refused(&span, ticket.id(), preceding_version));
        }
        append_timeline(&span, &envelope)?;
        span.commit().map_err(internal)?;
        Ok(())
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
    actual_behaviour: Option<String>,
    reporter_evidence: Option<String>,
    bug_qualification: Option<String>,
    bug_facts: Option<String>,
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
        actual_behaviour: row.get::<_, Option<String>>(10)?,
        reporter_evidence: row.get::<_, Option<String>>(11)?,
        bug_qualification: row.get::<_, Option<String>>(12)?,
        bug_facts: row.get::<_, Option<String>>(13)?,
        version: row.get::<_, i64>(14)?.unsigned_abs(),
    })
}

impl LoadedTicket {
    /// Assemble the aggregate. Every stored value passed validation on
    /// the way in, so a failure here is corruption the caller must
    /// hear about, not silently accept. A Bug row the 0020 schema
    /// wrote names no capture facts; it rehydrates with empty ones
    /// until an edit records them.
    fn rehydrate(&self) -> Result<Ticket, rusqlite::Error> {
        let body = match self.kind.as_str() {
            "implementation" => TicketBody::Implementation(ImplementationTicket::restore(
                SpecId::new(self.spec_id.ok_or_else(corrupt)?.unsigned_abs()),
                self.slice.clone().ok_or_else(corrupt)?,
                decode_criteria(&self.criteria)?,
            )),
            "bug" => TicketBody::Bug(Box::new(BugTicket::restore(
                self.title.clone().ok_or_else(corrupt)?,
                self.spec_id.map(|spec| SpecId::new(spec.unsigned_abs())),
                self.actual_behaviour.clone().unwrap_or_default(),
                self.reporter_evidence.clone().unwrap_or_default(),
                self.bug_qualification
                    .as_deref()
                    .map(decode_qualification)
                    .transpose()?,
                decode_facts(self.bug_facts.as_deref().unwrap_or("{}"))?,
            ))),
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
    actual_behaviour: Option<&'a str>,
    reporter_evidence: Option<&'a str>,
    bug_qualification: Option<String>,
    bug_facts: Option<String>,
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
                actual_behaviour: None,
                reporter_evidence: None,
                bug_qualification: None,
                bug_facts: None,
            },
            TicketBody::Bug(bug) => Self {
                kind: "bug",
                priority: priority.wire_name(),
                spec_id: bug.spec().map(|spec| spec.value() as i64),
                title: Some(bug.title()),
                slice: None,
                criteria: "[]".to_owned(),
                actual_behaviour: Some(bug.actual_behaviour()),
                reporter_evidence: Some(bug.reporter_evidence()),
                bug_qualification: bug.qualification().map(encode_qualification),
                bug_facts: Some(encode_facts(bug.facts())),
            },
            TicketBody::Task(task) => Self {
                kind: "task",
                priority: priority.wire_name(),
                spec_id: task.spec().map(|spec| spec.value() as i64),
                title: Some(task.title()),
                slice: None,
                criteria: "[]".to_owned(),
                actual_behaviour: None,
                reporter_evidence: None,
                bug_qualification: None,
                bug_facts: None,
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
        criteria.push(
            AcceptanceCriterion::new(row.outcome, decode_stories(row.stories)?)
                .map_err(|_| corrupt())?,
        );
    }
    Ok(criteria)
}

/// Decode one stored story-link list into the domain's story refs.
fn decode_stories(stored: Vec<StoredStory>) -> Result<Vec<UserStoryRef>, rusqlite::Error> {
    let mut stories = Vec::with_capacity(stored.len());
    for story in stored {
        let spec = SpecNumber::new(story.spec).map_err(|_| corrupt())?;
        stories.push(UserStoryRef::new(spec, story.story).map_err(|_| corrupt())?);
    }
    Ok(stories)
}

/// One qualification's stored form: the ten facts as plain data, so
/// rehydration needs no Project code.
#[derive(Serialize, Deserialize)]
struct StoredQualification {
    expected_behaviour: String,
    reproduction: String,
    environment: String,
    severity: String,
    frequency: String,
    affected_scope: String,
    risk: String,
    criteria: Vec<StoredCriterion>,
    verification_steps: Vec<StoredStep>,
}

/// One Verification Step's stored form.
#[derive(Serialize, Deserialize)]
struct StoredStep {
    command: String,
}

/// Encode the qualification the domain validated.
fn encode_qualification(qualification: &BugQualification) -> String {
    let stored = StoredQualification {
        expected_behaviour: qualification.expected_behaviour().to_owned(),
        reproduction: qualification.reproduction().to_owned(),
        environment: qualification.environment().to_owned(),
        severity: qualification.severity().wire_name().to_owned(),
        frequency: qualification.frequency().to_owned(),
        affected_scope: qualification.affected_scope().to_owned(),
        risk: qualification.risk().to_owned(),
        criteria: qualification
            .criteria()
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
            .collect(),
        verification_steps: qualification
            .verification_steps()
            .iter()
            .map(|step| StoredStep {
                command: step.command().to_owned(),
            })
            .collect(),
    };
    serde_json::to_string(&stored).expect("the qualification serialises")
}

/// Decode a stored qualification back into the domain's rule-valid
/// form.
fn decode_qualification(stored: &str) -> Result<BugQualification, rusqlite::Error> {
    let row: StoredQualification = serde_json::from_str(stored).map_err(|_| corrupt())?;
    let mut criteria = Vec::with_capacity(row.criteria.len());
    for criterion in row.criteria {
        criteria.push(
            AcceptanceCriterion::new(criterion.outcome, decode_stories(criterion.stories)?)
                .map_err(|_| corrupt())?,
        );
    }
    let mut steps = Vec::with_capacity(row.verification_steps.len());
    for step in row.verification_steps {
        steps.push(VerificationStep::new(step.command).map_err(|_| corrupt())?);
    }
    BugQualification::new(
        row.expected_behaviour,
        row.reproduction,
        row.environment,
        Severity::parse(&row.severity).ok_or_else(corrupt)?,
        row.frequency,
        row.affected_scope,
        row.risk,
        criteria,
        steps,
    )
    .map_err(|_| corrupt())
}

/// One Bug facts blob's stored form: the vendor-neutral collections,
/// evidence items by identity. Every field defaults empty so an
/// absent blob — a Bug row the 0020 schema wrote — decodes to the
/// empty facts it means.
#[derive(Serialize, Deserialize)]
struct StoredFacts {
    #[serde(default)]
    external_references: Vec<StoredReference>,
    #[serde(default)]
    occurrence_snapshots: Vec<StoredSnapshot>,
    #[serde(default)]
    evidence_items: Vec<u64>,
}

/// One External Reference's stored form.
#[derive(Serialize, Deserialize)]
struct StoredReference {
    uri: String,
    #[serde(default)]
    label: Option<String>,
}

/// One Occurrence Snapshot's stored form.
#[derive(Serialize, Deserialize)]
struct StoredSnapshot {
    observed_at: String,
    observation: String,
}

/// Encode the facts the domain validated.
fn encode_facts(facts: &BugFacts) -> String {
    let stored = StoredFacts {
        external_references: facts
            .external_references()
            .iter()
            .map(|reference| StoredReference {
                uri: reference.uri().to_owned(),
                label: reference.label().map(str::to_owned),
            })
            .collect(),
        occurrence_snapshots: facts
            .occurrence_snapshots()
            .iter()
            .map(|snapshot| StoredSnapshot {
                observed_at: snapshot.observed_at().to_owned(),
                observation: snapshot.observation().to_owned(),
            })
            .collect(),
        evidence_items: facts
            .evidence_items()
            .iter()
            .map(|item| item.value())
            .collect(),
    };
    serde_json::to_string(&stored).expect("the Bug facts serialise")
}

/// Decode a stored facts blob back into the domain's rule-valid form.
fn decode_facts(stored: &str) -> Result<BugFacts, rusqlite::Error> {
    let row: StoredFacts = serde_json::from_str(stored).map_err(|_| corrupt())?;
    let references = row
        .external_references
        .into_iter()
        .map(|reference| ExternalReference::new(reference.uri, reference.label))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| corrupt())?;
    let snapshots = row
        .occurrence_snapshots
        .into_iter()
        .map(|snapshot| OccurrenceSnapshot::new(snapshot.observed_at, snapshot.observation))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| corrupt())?;
    let items = row
        .evidence_items
        .into_iter()
        .map(EvidenceId::new)
        .collect();
    BugFacts::new(references, snapshots, items).map_err(|_| corrupt())
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

/// Why a guarded Ticket write was refused, read from the row's
/// current state.
fn ticket_write_refused(
    conn: &rusqlite::Connection,
    id: TicketId,
    attempted_from: u64,
) -> ApiError {
    match conn.query_row(
        "SELECT version FROM tickets WHERE id = ?1",
        params![id.value() as i64],
        |row| row.get::<_, i64>(0),
    ) {
        Ok(current) => ApiError::stale_version(attempted_from, current.unsigned_abs()),
        Err(rusqlite::Error::QueryReturnedNoRows) => ApiError::not_found(&format!("ticket {id}")),
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

    /// A quick-captured Bug body, standing alone.
    fn bug_body() -> Result<TicketBody, kanban_domain::TicketError> {
        TicketBody::bug(
            "Landing drops the integration branch",
            None,
            "The integration branch is dropped after a review lands.",
            "The landing log names the drop immediately after the merge.",
        )
    }

    /// One complete qualification for the fixture Bug.
    fn qualification() -> kanban_domain::BugQualification {
        kanban_domain::BugQualification::new(
            "The integration branch survives every landing.",
            "Re land a reviewed change; the branch list still names it.",
            "macOS 26, Kanban 0.1.0.",
            kanban_domain::Severity::Critical,
            "Every landing so far.",
            "All landing reviews.",
            "Duplicate landings and lost review state.",
            vec![
                kanban_domain::AcceptanceCriterion::new(
                    "The integration branch survives a landing.",
                    vec![story(1, 1)],
                )
                .expect("the fixture criterion links"),
            ],
            vec![
                kanban_domain::VerificationStep::new("cargo test -p kanban-storage tickets")
                    .expect("the fixture step carries its command"),
            ],
        )
        .expect("the fixture qualification is complete")
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
        let number = SpecNumber::new(project.mint(NumberKind::Spec).expect("active mints"))
            .expect("a minted number is positive");
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
        let number = TicketNumber::new(project.mint(NumberKind::Ticket).expect("active mints"))
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
            &bug_body().expect("the fixture body validates"),
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
            &bug_body().expect("the fixture body validates"),
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
    fn a_bugs_qualification_and_facts_round_trip_through_the_row() {
        let (_dir, database, store) = store();
        let (_project, _spec) = seeded_project_and_spec(&database);
        let created = created(
            &store,
            &database,
            Priority::Urgent,
            &bug_body().expect("the fixture body validates"),
        );

        // One command, one version bump, one save: the pattern the
        // application layer drives.
        let mut bug = created;
        bug.qualify(qualification()).expect("the Bug qualifies");
        store
            .save(
                &bug,
                transition(
                    bug.id(),
                    "qualified",
                    json!({ "severity": "critical", "version": bug.version() }),
                ),
            )
            .expect("the qualification saves");
        bug.record_bug_facts(
            kanban_domain::BugFacts::new(
                vec![
                    kanban_domain::ExternalReference::new(
                        "https://example.invalid/issues/12",
                        Some("The report".to_owned()),
                    )
                    .expect("the reference is a URI"),
                ],
                vec![
                    kanban_domain::OccurrenceSnapshot::new(
                        "2026-09-05T07:41:00Z",
                        "The log shows the drop.",
                    )
                    .expect("the snapshot carries its moment"),
                ],
                vec![kanban_domain::EvidenceId::new(2)],
            )
            .expect("the collections assemble"),
        )
        .expect("the Bug carries its facts");
        store
            .save(
                &bug,
                transition(
                    bug.id(),
                    "facts_recorded",
                    json!({ "version": bug.version() }),
                ),
            )
            .expect("the facts save");

        let found = store
            .find(bug.id())
            .expect("the find serves")
            .expect("the Ticket exists");
        let body = found.bug().expect("the Bug body rehydrates");
        assert_eq!(found.version(), 3);
        assert!(body.is_qualified());
        assert_eq!(body.severity(), Some(kanban_domain::Severity::Critical));
        let record = body.qualification().expect("the qualification stands");
        assert_eq!(
            record.expected_behaviour(),
            "The integration branch survives every landing."
        );
        assert_eq!(record.criteria()[0].stories(), [story(1, 1)].as_slice());
        assert_eq!(
            record.verification_steps()[0].command(),
            "cargo test -p kanban-storage tickets"
        );
        assert_eq!(
            body.facts().external_references()[0].uri(),
            "https://example.invalid/issues/12"
        );
        assert_eq!(
            body.facts().occurrence_snapshots()[0].observed_at(),
            "2026-09-05T07:41:00Z"
        );
        assert_eq!(
            body.facts().evidence_items(),
            [kanban_domain::EvidenceId::new(2)].as_slice()
        );
        assert_eq!(
            found.state(),
            kanban_domain::TicketState::Draft,
            "qualification and facts never move the state"
        );
    }

    #[test]
    fn a_stale_save_is_refused_without_a_row_move_or_a_timeline_append() {
        let (_dir, database, store) = store();
        let (_project, _spec) = seeded_project_and_spec(&database);
        let created = created(
            &store,
            &database,
            Priority::Normal,
            &bug_body().expect("the fixture body validates"),
        );
        let mut bug = created;
        bug.qualify(qualification()).expect("the fixture qualifies");
        store
            .save(
                &bug,
                transition(bug.id(), "qualified", json!({ "severity": "critical" })),
            )
            .expect("the first save lands");
        let timeline_before = ticket_timeline(&database).len();

        // The stored row has moved to version 2 while the saved
        // aggregate claims to save from version 1 again.
        let stale = bug;
        let error = store
            .save(
                &stale,
                transition(stale.id(), "qualified", json!({ "severity": "critical" })),
            )
            .expect_err("the stale save is refused");

        assert_eq!(error.code, ErrorCode::StaleVersion);
        let row: (i64, bool) = database
            .connection()
            .query_row(
                "SELECT version, bug_qualification IS NOT NULL FROM tickets WHERE id = ?1",
                rusqlite::params![stale.id().value() as i64],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("the row is readable");
        assert_eq!(
            row,
            (2, true),
            "the refused save must not move the row past the first save"
        );
        assert_eq!(
            ticket_timeline(&database).len(),
            timeline_before,
            "a stale save must not append a timeline row"
        );
    }

    #[test]
    fn a_row_the_0020_schema_wrote_rehydrates_with_empty_capture_facts() {
        let (_dir, mut database) = scratch_database();
        crate::migrations::apply_through(&database.connection(), 20)
            .expect("the 0020 schema applies");
        seeded_project_and_spec(&database);
        // The 0020 shape: a Bug named no capture columns, because they
        // did not exist yet.
        database
            .connection()
            .execute(
                "INSERT INTO tickets
                     (project_id, number, kind, priority, state, title, slice, criteria,
                      version)
                 VALUES (1, 7, 'bug', 'normal', 'draft', 'Legacy Bug', NULL, '[]', 1)",
                [],
            )
            .expect("the legacy row stands");

        let report = database
            .migrate(&AllowAllMigrations)
            .expect("the 0023 migration applies");
        assert_eq!(report.applied, vec![21, 22, 23]);

        let store = SqliteTicketStore::new(&database);
        let found = store
            .find(kanban_domain::TicketId::new(1))
            .expect("the find serves")
            .expect("the legacy row rehydrates");
        let body = found.bug().expect("the legacy Bug body rehydrates");
        assert_eq!(body.title(), "Legacy Bug");
        assert_eq!(
            body.actual_behaviour(),
            "",
            "a 0020 Bug names no capture facts; it rehydrates empty until edited"
        );
        assert_eq!(body.reporter_evidence(), "");
        assert!(!body.is_qualified());
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
            &bug_body().expect("the fixture body validates"),
        );
        let timeline_before = ticket_timeline(&database).len();

        let mut stale = project.clone();
        let number = TicketNumber::new(stale.mint(NumberKind::Ticket).expect("active mints"))
            .expect("a minted number is positive");
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
            &bug_body().expect("the fixture body validates"),
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
            &bug_body().expect("the fixture body validates"),
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
        // each violates its kind's schema. A Bug missing its capture
        // facts and a Task carrying them violate the shape triggers.
        for (sql, params) in [
            (
                "INSERT INTO tickets
                     (project_id, number, kind, priority, state, spec_id, title, slice,
                      criteria, actual_behaviour, reporter_evidence, bug_facts, version)
                 VALUES (1, 9, 'bug', 'normal', 'draft', NULL, 'A title', 'A slice', '[]',
                         'It drops.', 'The log.', '{}', 1)",
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
            (
                "INSERT INTO tickets
                     (project_id, number, kind, priority, state, spec_id, title,
                      criteria, bug_facts, version)
                 VALUES (1, 9, 'bug', 'normal', 'draft', NULL, 'A title', '[]', '{}', 1)",
                Vec::new(),
            ),
            (
                "INSERT INTO tickets
                     (project_id, number, kind, priority, state, spec_id, title,
                      criteria, actual_behaviour, version)
                 VALUES (1, 9, 'task', 'normal', 'draft', NULL, 'A title', '[]', 'It drops.', 1)",
                Vec::new(),
            ),
        ] {
            let outcome = if params.is_empty() {
                conn.execute(sql, [])
            } else {
                conn.execute(sql, rusqlite::params_from_iter(params))
            };
            let error = outcome.expect_err("the kind's schema is closed");
            assert!(
                error.to_string().contains("CHECK constraint failed")
                    || error.to_string().contains("exactly its own fields"),
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
        let number = TicketNumber::new(project.mint(NumberKind::Ticket).expect("active mints"))
            .expect("a minted number is positive");
        let body = bug_body().expect("the fixture body validates");

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
