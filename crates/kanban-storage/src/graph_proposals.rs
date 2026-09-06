//! The SQLite implementation of the Ticket graph proposal storage
//! port: rows in `ticket_graph_proposals` holding the complete Ticket
//! set and its dependency edges of one proposal against an approved
//! Spec version (DR-PS-16), with the closed proposed/approved
//! lifecycle. Recording lands the row and its timeline envelope in
//! one write; approval lands the proposal's move, every pinned
//! Ticket's `pinned_version` and version bump, and the approval's
//! timeline envelopes in one transaction, so a graph approval never
//! splits across a crash boundary. The partial UNIQUE index keeps one
//! approved graph per Spec version; the gate itself lives in the
//! domain.

use kanban_app::{GraphProposalStore, TimelineEnvelope};
use kanban_domain::{
    GraphProposalId, GraphProposalState, SpecId, Ticket, TicketDependency, TicketGraphProposal,
    TicketId,
};
use kanban_dto::ApiError;
use rusqlite::params;
use serde::{Deserialize, Serialize};

use crate::db::{ConnectionHandle, Database, WriteSpan};
use crate::timeline::insert_event;

/// Every stored column of one proposal row, in select order.
const PROPOSAL_COLUMNS: &str = "id, spec_id, spec_version, state, tickets, edges, version";

/// The graph proposal port over the authoritative database.
pub struct SqliteGraphProposalStore {
    conn: ConnectionHandle,
}

impl SqliteGraphProposalStore {
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

impl GraphProposalStore for SqliteGraphProposalStore {
    fn create(
        &self,
        spec: SpecId,
        spec_version: u64,
        tickets: Vec<TicketId>,
        edges: Vec<TicketDependency>,
        envelope: &dyn Fn(GraphProposalId) -> TimelineEnvelope,
    ) -> Result<TicketGraphProposal, ApiError> {
        let conn = self.lock();
        let span = WriteSpan::begin(&conn).map_err(internal)?;
        span.execute(
            "INSERT INTO ticket_graph_proposals
                 (spec_id, spec_version, state, tickets, edges, version)
             VALUES (?1, ?2, 'proposed', ?3, ?4, 1)",
            params![
                spec.value() as i64,
                spec_version as i64,
                encode_tickets(&tickets),
                encode_edges(&edges),
            ],
        )
        .map_err(internal)?;
        let id = GraphProposalId::new(
            span.last_insert_rowid()
                .try_into()
                .map_err(|_| ApiError::internal("the proposal identity overflowed"))?,
        );
        append_timeline(&span, &envelope(id))?;
        span.commit().map_err(internal)?;
        Ok(TicketGraphProposal::restore(
            id,
            spec,
            spec_version,
            tickets,
            edges,
            GraphProposalState::Proposed,
            1,
        ))
    }

    fn find(&self, id: GraphProposalId) -> Result<Option<TicketGraphProposal>, ApiError> {
        let conn = self.lock();
        let row = conn.query_row(
            &format!("SELECT {PROPOSAL_COLUMNS} FROM ticket_graph_proposals WHERE id = ?1"),
            params![id.value() as i64],
            load_proposal_row,
        );
        match row {
            Ok(loaded) => loaded.rehydrate().map(Some).map_err(internal),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(error) => Err(internal(error)),
        }
    }

    fn list(&self, spec: SpecId) -> Result<Vec<TicketGraphProposal>, ApiError> {
        let conn = self.lock();
        let mut statement = conn
            .prepare(&format!(
                "SELECT {PROPOSAL_COLUMNS} FROM ticket_graph_proposals
                 WHERE spec_id = ?1 ORDER BY id"
            ))
            .map_err(internal)?;
        let rows = statement
            .query_map(params![spec.value() as i64], load_proposal_row)
            .map_err(internal)?;
        let mut proposals = Vec::new();
        for row in rows {
            let loaded = row.map_err(internal)?;
            proposals.push(loaded.rehydrate().map_err(internal)?);
        }
        Ok(proposals)
    }

    fn apply_approval(
        &self,
        proposal: &TicketGraphProposal,
        pinned: &[Ticket],
        envelopes: &[TimelineEnvelope],
    ) -> Result<(), ApiError> {
        let conn = self.lock();
        let span = WriteSpan::begin(&conn).map_err(internal)?;
        // The proposal row moves under the guard of the version the
        // aggregate moved from; a unique violation names the version
        // that already carries an approved graph.
        let preceding = proposal.version() - 1;
        let changed = span
            .execute(
                "UPDATE ticket_graph_proposals
                 SET state = 'approved', version = ?2
                 WHERE id = ?1 AND version = ?3",
                params![
                    proposal.id().value() as i64,
                    proposal.version() as i64,
                    preceding as i64,
                ],
            )
            .map_err(unique_approval_refused)?;
        if changed != 1 {
            return Err(proposal_write_refused(&span, proposal.id(), preceding));
        }
        // Every pinned Ticket row moves beside it under the same
        // guard, or the whole approval rolls back.
        for ticket in pinned {
            let ticket_preceding = ticket.version() - 1;
            let changed = span
                .execute(
                    "UPDATE tickets
                     SET pinned_version = ?2, version = ?3
                     WHERE id = ?1 AND version = ?4 AND pinned_version IS NULL",
                    params![
                        ticket.id().value() as i64,
                        ticket
                            .pinned_version()
                            .expect("the approval pinned the Ticket")
                            as i64,
                        ticket.version() as i64,
                        ticket_preceding as i64,
                    ],
                )
                .map_err(internal)?;
            if changed != 1 {
                return Err(ticket_write_refused(&span, ticket.id(), ticket_preceding));
            }
        }
        for envelope in envelopes {
            append_timeline(&span, envelope)?;
        }
        span.commit().map_err(internal)?;
        Ok(())
    }
}

/// One decoded `ticket_graph_proposals` row before it is assembled.
struct LoadedProposal {
    id: u64,
    spec: u64,
    spec_version: u64,
    state: String,
    tickets: String,
    edges: String,
    version: u64,
}

/// Decode one `ticket_graph_proposals` row.
fn load_proposal_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<LoadedProposal> {
    Ok(LoadedProposal {
        id: row.get::<_, i64>(0)?.unsigned_abs(),
        spec: row.get::<_, i64>(1)?.unsigned_abs(),
        spec_version: row.get::<_, i64>(2)?.unsigned_abs(),
        state: row.get::<_, String>(3)?,
        tickets: row.get::<_, String>(4)?,
        edges: row.get::<_, String>(5)?,
        version: row.get::<_, i64>(6)?.unsigned_abs(),
    })
}

impl LoadedProposal {
    /// Assemble the record. Every stored value passed domain
    /// validation on the way in, so a failure here is corruption the
    /// caller must hear about, not silently accept.
    fn rehydrate(&self) -> Result<TicketGraphProposal, rusqlite::Error> {
        Ok(TicketGraphProposal::restore(
            GraphProposalId::new(self.id),
            SpecId::new(self.spec),
            self.spec_version,
            decode_tickets(&self.tickets)?,
            decode_edges(&self.edges)?,
            GraphProposalState::parse(&self.state).ok_or_else(corrupt)?,
            self.version,
        ))
    }
}

/// One stored Ticket set: identities alone, so rehydration needs no
/// join.
#[derive(Serialize, Deserialize)]
struct StoredTickets(Vec<u64>);

/// One stored edge: the blocking Ticket and the Ticket that waits.
#[derive(Serialize, Deserialize)]
struct StoredEdge {
    from: u64,
    to: u64,
}

/// Encode the Ticket set the domain validated.
fn encode_tickets(tickets: &[TicketId]) -> String {
    let stored = StoredTickets(tickets.iter().map(|ticket| ticket.value()).collect());
    serde_json::to_string(&stored).expect("the Ticket set serialises")
}

/// Encode the edges the domain validated.
fn encode_edges(edges: &[TicketDependency]) -> String {
    let stored: Vec<StoredEdge> = edges
        .iter()
        .map(|edge| StoredEdge {
            from: edge.from().value(),
            to: edge.to().value(),
        })
        .collect();
    serde_json::to_string(&stored).expect("the edges serialise")
}

/// Decode a stored Ticket set back into identities.
fn decode_tickets(stored: &str) -> Result<Vec<TicketId>, rusqlite::Error> {
    let rows: StoredTickets = serde_json::from_str(stored).map_err(|_| corrupt())?;
    Ok(rows.0.into_iter().map(TicketId::new).collect())
}

/// Decode stored edges back into the domain's edges.
fn decode_edges(stored: &str) -> Result<Vec<TicketDependency>, rusqlite::Error> {
    let rows: Vec<StoredEdge> = serde_json::from_str(stored).map_err(|_| corrupt())?;
    Ok(rows
        .into_iter()
        .map(|edge| TicketDependency::new(TicketId::new(edge.from), TicketId::new(edge.to)))
        .collect())
}

/// Why a guarded proposal write was refused, read from the row's
/// current state.
fn proposal_write_refused(
    conn: &rusqlite::Connection,
    id: GraphProposalId,
    attempted_from: u64,
) -> ApiError {
    match conn.query_row(
        "SELECT version FROM ticket_graph_proposals WHERE id = ?1",
        params![id.value() as i64],
        |row| row.get::<_, i64>(0),
    ) {
        Ok(current) => ApiError::stale_version(attempted_from, current.unsigned_abs()),
        Err(rusqlite::Error::QueryReturnedNoRows) => {
            ApiError::not_found(&format!("proposal {}", id.value()))
        }
        Err(error) => internal(error),
    }
}

/// Why a guarded Ticket write was refused, read from the row's
/// current state.
fn ticket_write_refused(
    conn: &rusqlite::Connection,
    id: kanban_domain::TicketId,
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

/// The unique-index violation a second approval of one Spec version
/// reports, in the gate's own words. SQLite names the partial index
/// or its columns; either way the violation is the version's second
/// approval.
fn unique_approval_refused(error: rusqlite::Error) -> ApiError {
    let message = error.to_string();
    if message.contains("UNIQUE constraint failed") && message.contains("ticket_graph_proposals") {
        return ApiError::invalid_request(
            "the Spec version already carries an approved Ticket graph",
        );
    }
    internal(error)
}

/// Report a SQLite failure the caller cannot act on.
fn internal(error: rusqlite::Error) -> ApiError {
    ApiError::internal(&error.to_string())
}

/// A stored proposal row failed domain validation.
#[derive(Debug)]
struct CorruptRow;

impl std::fmt::Display for CorruptRow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "a stored Ticket graph proposal row failed validation")
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
    use kanban_app::TimelineEnvelope;
    use kanban_domain::{
        GraphProposalId, GraphProposalState, Priority, Project, ProjectCounters,
        ProjectRegistration, ProjectState, SpecContent, SpecId, SpecNumber, Ticket, TicketBody,
        TicketDependency, TicketGraphProposal, TicketId, TicketNumber,
    };
    use kanban_dto::{TimelineEntityKind, TimelineEntityRef, TimelineEventKind};
    use serde_json::json;

    use super::SqliteGraphProposalStore;
    use crate::db::Database;
    use crate::migrations::AllowAllMigrations;
    use crate::projects::SqliteProjectStore;
    use crate::spec::SqliteSpecStore;
    use crate::test_support::scratch_database;
    use crate::tickets::SqliteTicketStore;
    use kanban_app::{GraphProposalStore, ProjectStore, SpecStore, TicketStore};

    fn story(spec: u64, ordinal: u64) -> kanban_domain::UserStoryRef {
        kanban_domain::UserStoryRef::new(
            SpecNumber::new(spec).expect("the fixture number is positive"),
            ordinal,
        )
        .expect("the fixture ordinal is positive")
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

    /// Seed the Project, the Spec, and two draft Tickets attached to
    /// it, returning the scratch world and the Spec identity.
    fn seeded() -> (
        tempfile::TempDir,
        Database,
        SqliteGraphProposalStore,
        SqliteTicketStore,
        SpecId,
    ) {
        let (dir, mut database) = scratch_database();
        database
            .migrate(&AllowAllMigrations)
            .expect("the migrations apply");
        let projects = SqliteProjectStore::new(&database);
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
        let specs = SqliteSpecStore::new(&database);
        let number = SpecNumber::new(
            project
                .mint(kanban_domain::NumberKind::Spec)
                .expect("active mints"),
        )
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
        let reloaded = projects
            .find(project.id())
            .expect("the reload serves")
            .expect("the Project exists");
        let tickets = SqliteTicketStore::new(&database);
        let mut moved = reloaded.clone();
        for _ in [0, 1] {
            let ticket_number = TicketNumber::new(
                moved
                    .mint(kanban_domain::NumberKind::Ticket)
                    .expect("active mints"),
            )
            .expect("a minted number is positive");
            tickets
                .create(
                    &moved,
                    ticket_number,
                    Priority::Normal,
                    &TicketBody::implementation(
                        Some(spec.id()),
                        spec.number(),
                        "Spec authoring creates content versions end to end",
                        vec![
                            kanban_domain::AcceptanceCriterion::new(
                                "Specs mint unique numbers.",
                                vec![story(1, 1)],
                            )
                            .expect("the fixture criterion links"),
                        ],
                    )
                    .expect("the fixture body validates"),
                    &|id| {
                        TimelineEnvelope::project(
                            1,
                            TimelineEventKind::Transition,
                            Some(TimelineEntityRef {
                                kind: TimelineEntityKind::Ticket,
                                id: id.value().to_string(),
                            }),
                            json!({ "action": "created", "id": id.value() }),
                        )
                    },
                )
                .expect("the fixture Ticket lands");
        }
        let store = SqliteGraphProposalStore::new(&database);
        (dir, database, store, tickets, spec.id())
    }

    /// The envelope the application layer builds for one proposal
    /// change, on the Spec.
    fn envelope(action: &str) -> TimelineEnvelope {
        TimelineEnvelope::project(
            1,
            TimelineEventKind::Transition,
            Some(TimelineEntityRef {
                kind: TimelineEntityKind::Spec,
                id: "1".to_owned(),
            }),
            json!({ "action": action }),
        )
    }

    /// One draft Ticket of the seeded world, by its identity.
    fn ticket(store: &SqliteTicketStore, id: u64) -> Ticket {
        store
            .find(TicketId::new(id))
            .expect("the find serves")
            .expect("the Ticket stands")
    }

    #[test]
    fn recording_lands_the_row_and_the_timeline_append() {
        let (_dir, _database, store, _tickets, spec) = seeded();

        let proposal = store
            .create(
                spec,
                1,
                vec![TicketId::new(1), TicketId::new(2)],
                vec![TicketDependency::new(TicketId::new(1), TicketId::new(2))],
                &|id| envelope(&format!("graph_proposed_{id}")),
            )
            .expect("the graph records");

        assert_eq!(proposal.id(), GraphProposalId::new(1));
        assert_eq!(*proposal.state(), GraphProposalState::Proposed);
        assert_eq!(proposal.version(), 1);
        let found = store
            .find(GraphProposalId::new(1))
            .expect("the find serves")
            .expect("the proposal stands");
        assert_eq!(found, proposal);
        assert_eq!(store.list(spec).expect("the list serves").len(), 1);
        assert!(
            store
                .list(SpecId::new(9))
                .expect("the list serves")
                .is_empty(),
            "another Spec's proposals stay out"
        );
    }

    #[test]
    fn approval_lands_the_state_the_pins_and_the_timeline_together() {
        let (_dir, database, store, tickets, spec) = seeded();
        let proposal = store
            .create(
                spec,
                1,
                vec![TicketId::new(1), TicketId::new(2)],
                Vec::new(),
                &|id| envelope(&format!("graph_proposed_{id}")),
            )
            .expect("the graph records");

        let mut approved = proposal.clone();
        approved.approve().expect("the gate approves");
        let mut pinned = Vec::new();
        let mut envelopes = vec![envelope("graph_approved")];
        for id in [1u64, 2] {
            let mut row = ticket(&tickets, id);
            row.pin_to(1).expect("the approval pins");
            envelopes.push(TimelineEnvelope::project(
                1,
                TimelineEventKind::Transition,
                Some(TimelineEntityRef {
                    kind: TimelineEntityKind::Ticket,
                    id: id.to_string(),
                }),
                json!({ "action": "pinned", "id": id, "spec_version": 1 }),
            ));
            pinned.push(row);
        }
        store
            .apply_approval(&approved, &pinned, &envelopes)
            .expect("the approval lands");

        let found = store
            .find(proposal.id())
            .expect("the find serves")
            .expect("the proposal stands");
        assert_eq!(*found.state(), GraphProposalState::Approved);
        assert_eq!(found.version(), 2);
        for id in [1u64, 2] {
            let row = ticket(&tickets, id);
            assert_eq!(row.pinned_version(), Some(1), "the pin persists");
            assert_eq!(row.version(), 2, "the pin is one applied change");
        }
        let conn = database.connection();
        let mut statement = conn
            .prepare(
                "SELECT detail FROM timeline_events
                 WHERE scope = 'project' AND entity_kind = 'ticket' ORDER BY id",
            )
            .expect("the timeline is readable");
        let details: Vec<String> = statement
            .query_map([], |row| row.get(0))
            .expect("the query runs")
            .collect::<Result<Vec<_>, _>>()
            .expect("the rows decode");
        let pins =
            details
                .iter()
                .filter(|detail| {
                    serde_json::from_str::<serde_json::Value>(detail)
                        .expect("stored detail is JSON")["action"]
                        == json!("pinned")
                })
                .count();
        assert_eq!(pins, 2, "every pin appended exactly one row");
    }

    #[test]
    fn a_stale_approval_rolls_the_whole_write_back() {
        let (_dir, database, store, tickets, spec) = seeded();
        let proposal = store
            .create(
                spec,
                1,
                vec![TicketId::new(1), TicketId::new(2)],
                Vec::new(),
                &|id| envelope(&format!("graph_proposed_{id}")),
            )
            .expect("the graph records");

        // Ticket 2 moved on its own: its stored row stands one version
        // ahead of the aggregate the approval pins from.
        database
            .connection()
            .execute("UPDATE tickets SET version = 2 WHERE id = 2", [])
            .expect("the row moves on its own");

        let mut approved = proposal.clone();
        approved.approve().expect("the gate approves");
        let mut pinned = Vec::new();
        for id in [1u64, 2] {
            let mut row = ticket(&tickets, id);
            if row.version() == 1 {
                row.pin_to(1).expect("the approval pins");
            } else {
                // The stale aggregate still claims version one.
                row = Ticket::restore(
                    row.id(),
                    row.project(),
                    row.number(),
                    row.priority(),
                    row.state(),
                    row.body().clone(),
                    row.profile().cloned(),
                    None,
                    1,
                );
                row.pin_to(1).expect("the stale aggregate pins");
            }
            pinned.push(row);
        }

        let error = store
            .apply_approval(&approved, &pinned, &[envelope("graph_approved")])
            .expect_err("the stale Ticket pin refuses the whole approval");

        assert_eq!(error.code, kanban_dto::ErrorCode::StaleVersion);
        let state: (i64, i64) = database
            .connection()
            .query_row(
                "SELECT (SELECT version FROM ticket_graph_proposals WHERE id = 1),
                        (SELECT COUNT(*) FROM tickets WHERE pinned_version IS NOT NULL)",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("the rows are readable");
        assert_eq!(
            state,
            (1, 0),
            "the rolled-back approval moved neither the proposal nor a pin"
        );
    }

    #[test]
    fn the_schema_keeps_one_approved_graph_per_spec_version() {
        let (_dir, _database, store, _tickets, spec) = seeded();
        let first = store
            .create(spec, 1, vec![TicketId::new(1)], Vec::new(), &|id| {
                envelope(&format!("graph_proposed_{id}"))
            })
            .expect("the first graph records");
        let second = store
            .create(spec, 1, vec![TicketId::new(2)], Vec::new(), &|id| {
                envelope(&format!("graph_proposed_{id}"))
            })
            .expect("the second graph records");

        let approved = |proposal: &TicketGraphProposal, pinned: &[Ticket]| {
            let mut moved = proposal.clone();
            moved.approve().expect("the gate approves");
            store.apply_approval(
                &moved,
                pinned,
                &[envelope("graph_approved"), envelope("pinned")],
            )
        };
        approved(&first, &[]).expect("the first approval lands");
        let error =
            approved(&second, &[]).expect_err("a second approval of the version is refused");
        assert_eq!(error.code, kanban_dto::ErrorCode::InvalidRequest);
        assert_eq!(
            error.message,
            "the Spec version already carries an approved Ticket graph"
        );
    }

    #[test]
    fn the_store_serves_through_a_shared_connection() {
        let (_dir, _database, store, _tickets, spec) = seeded();
        let boxed: Box<dyn GraphProposalStore> = Box::new(store);

        let served = std::thread::spawn(move || {
            boxed
                .create(spec, 1, vec![TicketId::new(1)], Vec::new(), &|_| {
                    envelope("graph_proposed")
                })
                .map(|proposal| proposal.id().value())
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
