//! The SQLite implementation of the Ticket dependency storage port:
//! rows in `ticket_dependencies` naming the registered edges — which
//! may cross Specs and Projects, so they name Tickets alone — and
//! rows in `ticket_blockers` carrying the explicit external blockers
//! of unregistered waiting work (DR-DE-02, DR-DE-04). Every change
//! moves the waiting Ticket's aggregate version under its optimistic
//! guard and lands the application's timeline envelope unchanged in
//! the same transaction, so an edge, a blocker, and the version that
//! orders them never split across a crash boundary. Cycle rejection
//! and the readiness projection live in the domain; this module only
//! rehydrates what it stored.

use kanban_app::{DependencyStore, TimelineEnvelope};
use kanban_domain::{
    BlockerDescription, ExternalBlocker, ExternalBlockerId, Ticket, TicketDependency, TicketId,
};
use kanban_dto::ApiError;
use rusqlite::params;

use crate::db::{ConnectionHandle, Database, WriteSpan};
use crate::timeline::insert_event;

/// The dependency port over the authoritative database.
pub struct SqliteDependencyStore {
    conn: ConnectionHandle,
}

impl SqliteDependencyStore {
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

impl DependencyStore for SqliteDependencyStore {
    fn add_dependency(
        &self,
        waiting: &Ticket,
        edge: TicketDependency,
        envelope: &dyn Fn() -> TimelineEnvelope,
    ) -> Result<Ticket, ApiError> {
        let conn = self.lock();
        let span = WriteSpan::begin(&conn).map_err(internal)?;
        bump_waiting_version(&span, waiting)?;
        span.execute(
            "INSERT INTO ticket_dependencies (from_ticket, to_ticket) VALUES (?1, ?2)",
            params![edge.from().value() as i64, edge.to().value() as i64,],
        )
        .map_err(internal)?;
        append_timeline(&span, &envelope())?;
        span.commit().map_err(internal)?;
        Ok(moved(waiting))
    }

    fn remove_dependency(
        &self,
        waiting: &Ticket,
        edge: TicketDependency,
        envelope: &dyn Fn() -> TimelineEnvelope,
    ) -> Result<Ticket, ApiError> {
        let conn = self.lock();
        let span = WriteSpan::begin(&conn).map_err(internal)?;
        bump_waiting_version(&span, waiting)?;
        let removed = span
            .execute(
                "DELETE FROM ticket_dependencies WHERE from_ticket = ?1 AND to_ticket = ?2",
                params![edge.from().value() as i64, edge.to().value() as i64,],
            )
            .map_err(internal)?;
        if removed != 1 {
            return Err(ApiError::internal(
                "the dependency disappeared before its removal",
            ));
        }
        append_timeline(&span, &envelope())?;
        span.commit().map_err(internal)?;
        Ok(moved(waiting))
    }

    fn add_blocker(
        &self,
        waiting: &Ticket,
        description: &BlockerDescription,
        envelope: &dyn Fn(ExternalBlockerId) -> TimelineEnvelope,
    ) -> Result<(Ticket, ExternalBlocker), ApiError> {
        let conn = self.lock();
        let span = WriteSpan::begin(&conn).map_err(internal)?;
        bump_waiting_version(&span, waiting)?;
        span.execute(
            "INSERT INTO ticket_blockers (ticket_id, description) VALUES (?1, ?2)",
            params![waiting.id().value() as i64, description.as_str()],
        )
        .map_err(internal)?;
        let id = ExternalBlockerId::new(
            span.last_insert_rowid()
                .try_into()
                .map_err(|_| ApiError::internal("the blocker identity overflowed"))?,
        );
        append_timeline(&span, &envelope(id))?;
        span.commit().map_err(internal)?;
        let blocker = ExternalBlocker::restore(id, waiting.id(), description.clone());
        Ok((moved(waiting), blocker))
    }

    fn remove_blocker(
        &self,
        waiting: &Ticket,
        blocker: ExternalBlocker,
        envelope: &dyn Fn() -> TimelineEnvelope,
    ) -> Result<Ticket, ApiError> {
        let conn = self.lock();
        let span = WriteSpan::begin(&conn).map_err(internal)?;
        bump_waiting_version(&span, waiting)?;
        let removed = span
            .execute(
                "DELETE FROM ticket_blockers WHERE id = ?1",
                params![blocker.id().value() as i64],
            )
            .map_err(internal)?;
        if removed != 1 {
            return Err(ApiError::internal(
                "the blocker disappeared before its removal",
            ));
        }
        append_timeline(&span, &envelope())?;
        span.commit().map_err(internal)?;
        Ok(moved(waiting))
    }

    fn list_dependencies(&self) -> Result<Vec<TicketDependency>, ApiError> {
        let conn = self.lock();
        let mut statement = conn
            .prepare("SELECT from_ticket, to_ticket FROM ticket_dependencies ORDER BY id")
            .map_err(internal)?;
        let rows = statement.query_map([], load_edge_row).map_err(internal)?;
        let mut edges = Vec::new();
        for row in rows {
            let (from, to) = row.map_err(internal)?;
            edges.push(TicketDependency::new(
                TicketId::new(from),
                TicketId::new(to),
            ));
        }
        Ok(edges)
    }

    fn blockers_of(&self, ticket: TicketId) -> Result<Vec<ExternalBlocker>, ApiError> {
        let conn = self.lock();
        let mut statement = conn
            .prepare("SELECT id, description FROM ticket_blockers WHERE ticket_id = ?1 ORDER BY id")
            .map_err(internal)?;
        let rows = statement
            .query_map(params![ticket.value() as i64], load_blocker_row)
            .map_err(internal)?;
        let mut blockers = Vec::new();
        for row in rows {
            let (id, description) = row.map_err(internal)?;
            blockers.push(ExternalBlocker::restore(
                ExternalBlockerId::new(id),
                ticket,
                BlockerDescription::new(&description)
                    .map_err(|error| ApiError::internal(&error.to_string()))?,
            ));
        }
        Ok(blockers)
    }
}

/// One decoded `ticket_dependencies` row.
fn load_edge_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<(u64, u64)> {
    Ok((
        row.get::<_, i64>(0)?.unsigned_abs(),
        row.get::<_, i64>(1)?.unsigned_abs(),
    ))
}

/// The waiting Ticket as one applied change leaves it: identical,
/// with its aggregate version moved forward by one.
fn moved(waiting: &Ticket) -> Ticket {
    Ticket::restore(
        waiting.id(),
        waiting.project(),
        waiting.number(),
        waiting.priority(),
        waiting.state(),
        waiting.body().clone(),
        waiting.version() + 1,
    )
}

/// Move the waiting Ticket's aggregate version forward by one under
/// its optimistic guard. The application layer checked the version
/// before applying, so a refusal here means a concurrent writer
/// moved it first, and nothing this span wrote may land.
fn bump_waiting_version(span: &WriteSpan<'_>, waiting: &Ticket) -> Result<(), ApiError> {
    let expected = waiting.version();
    let changed = span
        .execute(
            "UPDATE tickets SET version = ?2 WHERE id = ?1 AND version = ?3",
            params![
                waiting.id().value() as i64,
                expected as i64 + 1,
                expected as i64,
            ],
        )
        .map_err(internal)?;
    if changed != 1 {
        return Err(ticket_write_refused(span, waiting.id(), expected));
    }
    Ok(())
}

/// Why a guarded Ticket write was refused, read from the row's
/// current state.
fn ticket_write_refused(span: &WriteSpan<'_>, id: TicketId, attempted_from: u64) -> ApiError {
    match span.query_row(
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

/// Insert the application's envelope, unchanged, on the same
/// transaction as the row it records.
fn append_timeline(span: &WriteSpan<'_>, envelope: &TimelineEnvelope) -> Result<(), ApiError> {
    insert_event(span, envelope).map_err(|error| ApiError::internal(&error.to_string()))
}

/// One decoded `ticket_blockers` row.
fn load_blocker_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<(u64, String)> {
    Ok((
        row.get::<_, i64>(0)?.unsigned_abs(),
        row.get::<_, String>(1)?,
    ))
}

#[cfg(test)]
mod tests {
    use kanban_app::{DependencyStore, ProjectStore, TicketStore, TimelineEnvelope};
    use kanban_domain::{
        BlockerDescription, NumberKind, Priority, Project, ProjectId, ProjectRegistration,
        TicketBody, TicketId, TicketNumber,
    };
    use kanban_dto::{ErrorCode, TimelineEntityKind, TimelineEntityRef, TimelineEventKind};
    use rusqlite::params;
    use serde_json::json;

    use super::SqliteDependencyStore;
    use crate::db::Database;
    use crate::migrations::AllowAllMigrations;
    use crate::projects::SqliteProjectStore;
    use crate::test_support::scratch_database;
    use crate::tickets::SqliteTicketStore;

    fn registration(code: &str) -> ProjectRegistration {
        ProjectRegistration::new(
            code,
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

    /// One stored Project, refusing a fixture that seeded nothing.
    fn seeded_project(database: &Database, project_id: u64) -> Project {
        SqliteProjectStore::new(database)
            .find(ProjectId::new(project_id))
            .expect("the reload serves")
            .unwrap_or_else(|| panic!("the fixture Project {project_id} must be seeded first"))
    }

    /// Seed the two Projects the cross-Project tests write against.
    fn seed_projects(database: &Database) {
        let projects = SqliteProjectStore::new(database);
        for (id, code) in [(1, "CORE"), (2, "EDGE")] {
            projects
                .create(&registration(code), &|assigned| {
                    TimelineEnvelope::project(
                        id,
                        TimelineEventKind::Transition,
                        Some(TimelineEntityRef {
                            kind: TimelineEntityKind::Project,
                            id: assigned.value().to_string(),
                        }),
                        json!({ "action": "registered", "id": assigned.value() }),
                    )
                })
                .expect("the fixture Project lands");
        }
    }

    /// Create one Ticket through the ticket port, minting its number
    /// on the stored Project aggregate.
    fn created_ticket(database: &Database, project_id: u64, title: &str) -> kanban_domain::Ticket {
        let tickets = SqliteTicketStore::new(database);
        let mut project = seeded_project(database, project_id);
        let number = TicketNumber::new(project.mint(NumberKind::Ticket).expect("active mints"))
            .expect("a minted number is positive");
        tickets
            .create(
                &project,
                number,
                Priority::Normal,
                &TicketBody::bug(title, None).expect("the fixture body validates"),
                &|id| {
                    TimelineEnvelope::project(
                        project_id,
                        TimelineEventKind::Transition,
                        Some(TimelineEntityRef {
                            kind: TimelineEntityKind::Ticket,
                            id: id.value().to_string(),
                        }),
                        json!({ "action": "created", "id": id.value() }),
                    )
                },
            )
            .expect("the fixture Ticket lands")
    }

    /// The envelope the application layer builds for one dependency
    /// change, on the waiting Ticket's Project timeline.
    fn transition(
        project: u64,
        ticket: TicketId,
        action: &str,
        facts: serde_json::Value,
    ) -> TimelineEnvelope {
        let mut detail = facts;
        let object = detail.as_object_mut().expect("the facts are an object");
        object.insert("action".to_owned(), serde_json::Value::from(action));
        object.insert("id".to_owned(), serde_json::Value::from(ticket.value()));
        TimelineEnvelope::project(
            project,
            TimelineEventKind::Transition,
            Some(TimelineEntityRef {
                kind: TimelineEntityKind::Ticket,
                id: ticket.value().to_string(),
            }),
            detail,
        )
    }

    /// The waiting Ticket as the store should return it: identical,
    /// with its version moved forward by one.
    fn moved(ticket: &kanban_domain::Ticket) -> kanban_domain::Ticket {
        kanban_domain::Ticket::restore(
            ticket.id(),
            ticket.project(),
            ticket.number(),
            ticket.priority(),
            ticket.state(),
            ticket.body().clone(),
            ticket.version() + 1,
        )
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
    fn edges_and_blockers_round_trip() {
        let (_dir, _database, store, core_ticket, edge_ticket) = harness();

        let waiting = store
            .add_dependency(
                &edge_ticket,
                kanban_domain::TicketDependency::new(core_ticket.id(), edge_ticket.id()),
                &|| transition(2, edge_ticket.id(), "dependency_added", json!({})),
            )
            .expect("the edge lands");

        assert_eq!(waiting.version(), edge_ticket.version() + 1);
        assert_eq!(
            store.list_dependencies().expect("the graph serves"),
            vec![kanban_domain::TicketDependency::new(
                core_ticket.id(),
                edge_ticket.id()
            )],
            "a cross-Project edge rehydrates from the rows"
        );

        let (_, blocker) = store
            .add_blocker(
                &waiting,
                &BlockerDescription::new("The vendor SDK 4 upgrade")
                    .expect("the fixture description validates"),
                &|id| {
                    transition(
                        2,
                        edge_ticket.id(),
                        "blocker_added",
                        json!({ "id": id.value() }),
                    )
                },
            )
            .expect("the blocker lands");
        assert_eq!(blocker.id().value(), 1);
        assert_eq!(blocker.ticket(), edge_ticket.id());
        assert_eq!(blocker.description().as_str(), "The vendor SDK 4 upgrade");
        assert_eq!(
            store
                .blockers_of(edge_ticket.id())
                .expect("the blockers serve"),
            vec![blocker.clone()]
        );
        assert!(
            store
                .blockers_of(core_ticket.id())
                .expect("the blockers serve")
                .is_empty(),
            "another Ticket's blockers stay out"
        );
    }

    #[test]
    fn removing_edges_and_blockers_clears_the_rows() {
        let (_dir, _database, store, core_ticket, edge_ticket) = harness();
        let edge = kanban_domain::TicketDependency::new(core_ticket.id(), edge_ticket.id());
        let with_edge = store
            .add_dependency(&edge_ticket, edge, &|| {
                transition(2, edge_ticket.id(), "dependency_added", json!({}))
            })
            .expect("the edge lands");
        let (moved, _) = store
            .add_blocker(
                &with_edge,
                &BlockerDescription::new("Design sign-off").expect("the fixture validates"),
                &|id| {
                    transition(
                        2,
                        edge_ticket.id(),
                        "blocker_added",
                        json!({ "id": id.value() }),
                    )
                },
            )
            .expect("the blocker lands");

        let without_edge = store
            .remove_dependency(&moved, edge, &|| {
                transition(2, edge_ticket.id(), "dependency_removed", json!({}))
            })
            .expect("the edge leaves");
        store
            .remove_blocker(
                &without_edge,
                blocker_stored(&store, edge_ticket.id()),
                &|| transition(2, edge_ticket.id(), "blocker_removed", json!({})),
            )
            .expect("the blocker leaves");

        assert!(
            store
                .list_dependencies()
                .expect("the graph serves")
                .is_empty()
        );
        assert!(
            store
                .blockers_of(edge_ticket.id())
                .expect("the blockers serve")
                .is_empty()
        );
    }

    /// The blocker the store recorded against `ticket`.
    fn blocker_stored(
        store: &SqliteDependencyStore,
        ticket: TicketId,
    ) -> kanban_domain::ExternalBlocker {
        store
            .blockers_of(ticket)
            .expect("the blockers serve")
            .into_iter()
            .next()
            .expect("the fixture blocker is recorded")
    }

    #[test]
    fn a_change_lands_the_row_the_version_and_the_timeline_append_together() {
        let (_dir, database, store, core_ticket, edge_ticket) = harness();

        let waiting = store
            .add_dependency(
                &edge_ticket,
                kanban_domain::TicketDependency::new(core_ticket.id(), edge_ticket.id()),
                &|| {
                    transition(
                        2,
                        edge_ticket.id(),
                        "dependency_added",
                        json!({ "from_ticket": core_ticket.id().value() }),
                    )
                },
            )
            .expect("the edge lands");

        assert_eq!(waiting, moved(&edge_ticket));
        let stored: (i64, i64) = database
            .connection()
            .query_row(
                "SELECT (SELECT version FROM tickets WHERE id = ?1),
                        (SELECT COUNT(*) FROM ticket_dependencies)",
                params![edge_ticket.id().value() as i64],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("the rows are readable");
        assert_eq!(
            stored,
            (edge_ticket.version() as i64 + 1, 1),
            "the version move and the edge row land together"
        );
        assert_eq!(
            ticket_timeline(&database).last().expect("the row appended"),
            &json!({
                "action": "dependency_added",
                "id": edge_ticket.id().value(),
                "from_ticket": core_ticket.id().value(),
            }),
            "the envelope reaches the waiting Ticket's own timeline unchanged"
        );
    }

    #[test]
    fn a_stale_change_is_refused_without_a_row_or_a_timeline_append() {
        let (_dir, database, store, core_ticket, edge_ticket) = harness();
        let timeline_before = ticket_timeline(&database).len();

        // One landed change moves the Ticket's row past the fresh
        // aggregate the stale change guards on.
        let (moved, _) = store
            .add_blocker(
                &edge_ticket,
                &BlockerDescription::new("Design sign-off").expect("the fixture validates"),
                &|id| {
                    transition(
                        2,
                        edge_ticket.id(),
                        "blocker_added",
                        json!({ "id": id.value() }),
                    )
                },
            )
            .expect("the first change lands");
        assert_eq!(moved.version(), edge_ticket.version() + 1);

        let error = store
            .add_dependency(
                &edge_ticket,
                kanban_domain::TicketDependency::new(core_ticket.id(), edge_ticket.id()),
                &|| transition(2, edge_ticket.id(), "dependency_added", json!({})),
            )
            .expect_err("the stale aggregate is refused");
        assert_eq!(error.code, ErrorCode::StaleVersion);
        let edges: i64 = database
            .connection()
            .query_row("SELECT COUNT(*) FROM ticket_dependencies", [], |row| {
                row.get(0)
            })
            .expect("the rows are readable");
        assert_eq!(edges, 0, "a stale change must not land a row");
        assert_eq!(
            ticket_timeline(&database).len(),
            timeline_before + 1,
            "only the first change appended"
        );
    }

    #[test]
    fn the_schema_keeps_edges_registered_and_blockers_explicit() {
        let (_dir, database, _store, core_ticket, edge_ticket) = harness();
        let conn = database.connection();

        // A self edge, an edge naming no registered Ticket, and a
        // blank blocker: each violates its own rule.
        for sql in [
            format!(
                "INSERT INTO ticket_dependencies (from_ticket, to_ticket)
                 VALUES ({id}, {id})",
                id = core_ticket.id().value()
            ),
            "INSERT INTO ticket_dependencies (from_ticket, to_ticket)
             VALUES (1, 99)"
                .to_owned(),
            "INSERT INTO ticket_blockers (ticket_id, description) VALUES (1, '  ')".to_owned(),
        ] {
            let outcome = conn.execute(&sql, []);
            assert!(
                outcome.is_err(),
                "`{sql}` should refuse, got: {:?}",
                outcome.map(|changed| changed.to_string())
            );
        }

        // The duplicate edge rule holds after a legal edge lands.
        conn.execute(
            "INSERT INTO ticket_dependencies (from_ticket, to_ticket) VALUES (?1, ?2)",
            params![
                core_ticket.id().value() as i64,
                edge_ticket.id().value() as i64
            ],
        )
        .expect("the legal edge lands");
        let outcome = conn.execute(
            "INSERT INTO ticket_dependencies (from_ticket, to_ticket) VALUES (?1, ?2)",
            params![
                core_ticket.id().value() as i64,
                edge_ticket.id().value() as i64
            ],
        );
        let error = outcome.expect_err("an edge registers once");
        assert!(
            error.to_string().contains("UNIQUE constraint failed"),
            "the schema should enforce one edge per pair: {error}"
        );
    }

    #[test]
    fn the_store_serves_through_a_shared_connection() {
        let (_dir, database, store, core_ticket, edge_ticket) = harness();
        let boxed: Box<dyn DependencyStore> = Box::new(store);
        let edge = kanban_domain::TicketDependency::new(core_ticket.id(), edge_ticket.id());
        let threaded = edge_ticket.clone();
        let waiting_id = edge_ticket.id();

        let served = std::thread::spawn(move || {
            boxed
                .add_dependency(&threaded, edge, &|| {
                    transition(2, waiting_id, "dependency_added", serde_json::json!({}))
                })
                .map(|ticket| ticket.version())
        })
        .join()
        .expect("the serving thread finishes");

        assert_eq!(
            served.expect("the threaded change lands"),
            edge_ticket_version(&database, edge_ticket.id()),
            "the port is Send + Sync over one connection"
        );
    }

    /// The version one stored Ticket row holds.
    fn edge_ticket_version(database: &Database, id: TicketId) -> u64 {
        database
            .connection()
            .query_row(
                "SELECT version FROM tickets WHERE id = ?1",
                params![id.value() as i64],
                |row| row.get::<_, i64>(0),
            )
            .expect("the version is readable")
            .unsigned_abs()
    }

    /// A migrated database, its dependency store, and the two Tickets
    /// the cross-Project fixtures use: Ticket 1 of CORE and Ticket 2
    /// of EDGE, both fresh drafts at version 1.
    fn harness() -> (
        tempfile::TempDir,
        Database,
        SqliteDependencyStore,
        kanban_domain::Ticket,
        kanban_domain::Ticket,
    ) {
        let (dir, mut database) = scratch_database();
        database
            .migrate(&AllowAllMigrations)
            .expect("the migrations apply");
        seed_projects(&database);
        let core_ticket = created_ticket(&database, 1, "Landing drops the integration branch");
        let edge_ticket = created_ticket(&database, 2, "Archive the old register");
        let store = SqliteDependencyStore::new(&database);
        (dir, database, store, core_ticket, edge_ticket)
    }
}
