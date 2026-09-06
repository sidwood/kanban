//! SQLite run-scoped capabilities (KAN-S9): the durable minted rows
//! a won Dispatch Request claim leaves behind, read back for
//! enforcement and expired by run settlement. The mint itself rides
//! the dispatch claim's transaction
//! ([`insert_capability`](insert_capability)); this module owns the
//! row's shape both sides of that transaction.

use kanban_app::{CapabilityMintDraft, CapabilityStore, TimelineEnvelope};
use kanban_domain::{
    Capability, CapabilityId, CapabilityRole, CapabilityScope, CapabilityStatus, McpOperations,
    ReviewerSlotId,
};
use kanban_dto::ApiError;
use rusqlite::params;

use crate::db::{ConnectionHandle, Database, WriteSpan};
use crate::timeline::insert_event;

const CAPABILITY_COLUMNS: &str = "id, dispatch_request_id, ticket_id, lane_id, role,
                                 reviewer_slot_id, operations, status, minted_at,
                                 settled_at";

/// SQLite-backed run-scoped capabilities.
pub struct SqliteCapabilityStore {
    conn: ConnectionHandle,
}

impl SqliteCapabilityStore {
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

impl CapabilityStore for SqliteCapabilityStore {
    fn find(&self, id: CapabilityId) -> Result<Option<Capability>, ApiError> {
        let conn = self.lock();
        match conn.query_row(
            &format!("SELECT {CAPABILITY_COLUMNS} FROM capabilities WHERE id = ?1"),
            params![id.value() as i64],
            decode_capability,
        ) {
            Ok(capability) => Ok(Some(capability)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(error) => Err(internal(error)),
        }
    }

    fn settle(
        &self,
        id: CapabilityId,
        settled_at: u64,
        envelope: TimelineEnvelope,
    ) -> Result<Capability, ApiError> {
        let conn = self.lock();
        let span = WriteSpan::begin(&conn).map_err(internal)?;
        let outcome = Self::settle_inside(&span, id, settled_at, envelope);
        match outcome {
            Ok(capability) => {
                span.commit().map_err(internal)?;
                Ok(capability)
            }
            Err(error) => Err(error),
        }
    }
}

impl SqliteCapabilityStore {
    /// Settle `id` inside a span the caller commits: the expiry and
    /// its timeline row land together or not at all. The first
    /// settlement keeps its timestamp; settling again changes
    /// nothing, because recovery is idempotent and renewal does not
    /// exist.
    fn settle_inside(
        span: &WriteSpan<'_>,
        id: CapabilityId,
        settled_at: u64,
        envelope: TimelineEnvelope,
    ) -> Result<Capability, ApiError> {
        let changed = span
            .execute(
                "UPDATE capabilities
                 SET status = 'settled', settled_at = COALESCE(settled_at, ?2)
                 WHERE id = ?1",
                params![id.value() as i64, settled_at as i64],
            )
            .map_err(internal)?;
        if changed != 1 {
            return Err(ApiError::not_found(&format!("capability {}", id.value())));
        }
        insert_event(span, &envelope).map_err(internal)?;
        span.query_row(
            &format!("SELECT {CAPABILITY_COLUMNS} FROM capabilities WHERE id = ?1"),
            params![id.value() as i64],
            decode_capability,
        )
        .map_err(internal)
    }
}

/// Mint `draft`'s row inside the dispatch claim's transaction, so the
/// claim and the authority it grants land together or not at all.
/// The row-level UNIQUE dispatch binding is the durable refusal of
/// renewal: one Dispatch Request, one capability, never a second.
pub(crate) fn insert_capability(
    span: &WriteSpan<'_>,
    mint: &CapabilityMintDraft,
) -> Result<Capability, ApiError> {
    let operations: Vec<&str> = mint.operations().iter().collect();
    let operations = serde_json::to_string(&operations).map_err(|error| {
        ApiError::internal(&format!("the permitted operations cannot encode: {error}"))
    })?;
    let scope = mint.scope();
    let inserted = span.execute(
        "INSERT INTO capabilities
             (dispatch_request_id, ticket_id, lane_id, role, reviewer_slot_id,
              operations, status, minted_at, settled_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'active', ?7, NULL)",
        params![
            mint.dispatch().value() as i64,
            scope.ticket().value() as i64,
            scope.lane().value() as i64,
            scope.role().wire_name(),
            scope.reviewer_slot().map(|slot| slot.value() as i64),
            operations,
            mint.minted_at() as i64,
        ],
    );
    match inserted {
        Ok(_) => {}
        Err(error) if is_duplicate_dispatch(&error) => {
            return Err(ApiError::invalid_request(
                "the Dispatch Request already minted its capability; a settled run mints \
                 nothing again",
            ));
        }
        Err(error) => return Err(internal(error)),
    }
    let id = CapabilityId::new(
        span.last_insert_rowid()
            .try_into()
            .map_err(|_| ApiError::internal("the capability identity overflowed"))?,
    );
    Capability::mint(
        id,
        mint.dispatch(),
        scope.clone(),
        mint.operations().clone(),
        mint.minted_at(),
    )
    .map_err(|error| ApiError::invalid_request(&error.to_string()))
}

fn decode_capability(row: &rusqlite::Row<'_>) -> rusqlite::Result<Capability> {
    let role = CapabilityRole::parse(&row.get::<_, String>(4)?).ok_or_else(|| {
        rusqlite::Error::InvalidColumnType(4, "role".to_owned(), rusqlite::types::Type::Text)
    })?;
    let status = CapabilityStatus::parse(&row.get::<_, String>(7)?).ok_or_else(|| {
        rusqlite::Error::InvalidColumnType(7, "status".to_owned(), rusqlite::types::Type::Text)
    })?;
    let stored_operations: String = row.get(6)?;
    let operations: Vec<String> = serde_json::from_str(&stored_operations).map_err(|_| {
        rusqlite::Error::InvalidColumnType(6, "operations".to_owned(), rusqlite::types::Type::Text)
    })?;
    let reviewer_slot = row
        .get::<_, Option<i64>>(5)?
        .map(|slot| ReviewerSlotId::new(slot as u64));
    Ok(Capability::restore(
        CapabilityId::new(row.get::<_, i64>(0)? as u64),
        kanban_domain::DispatchRequestId::new(row.get::<_, i64>(1)? as u64),
        CapabilityScope::restore(
            kanban_domain::TicketId::new(row.get::<_, i64>(2)? as u64),
            kanban_domain::LaneId::new(row.get::<_, i64>(3)? as u64),
            role,
            reviewer_slot,
        ),
        McpOperations::restore(operations),
        status,
        row.get::<_, i64>(8)? as u64,
        row.get::<_, Option<i64>>(9)?.map(|moment| moment as u64),
    ))
}

fn is_duplicate_dispatch(error: &rusqlite::Error) -> bool {
    error
        .to_string()
        .contains("UNIQUE constraint failed: capabilities.dispatch_request_id")
}

fn internal(error: impl ToString) -> ApiError {
    ApiError::internal(&error.to_string())
}

#[cfg(test)]
mod tests {
    use kanban_app::{CapabilityMintDraft, CapabilityStore, DispatchStore as _};
    use kanban_domain::{
        Capability, CapabilityId, CapabilityRole, CapabilityScope, CapabilityStatus,
        DispatchRequestId, LaneId, McpOperations, TicketId,
    };
    use kanban_dto::{TimelineEntityKind, TimelineEntityRef, TimelineEventKind};
    use serde_json::json;

    use super::SqliteCapabilityStore;
    use crate::db::WriteSpan;
    use crate::migrations::AllowAllMigrations;
    use crate::{Database, SqliteDispatchStore};
    use kanban_app::TimelineEnvelope;

    fn database() -> (tempfile::TempDir, Database) {
        let dir = tempfile::tempdir().expect("a scratch directory is available");
        let mut database = Database::open(&dir.path().join("kanban.sqlite"))
            .expect("opening a fresh database succeeds");
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
        (dir, database)
    }

    fn envelope(capability: CapabilityId, action: &str) -> TimelineEnvelope {
        TimelineEnvelope::project(
            1,
            TimelineEventKind::Transition,
            Some(TimelineEntityRef {
                kind: TimelineEntityKind::Ticket,
                id: "1".to_owned(),
            }),
            json!({ "action": action, "capability_id": capability.value() }),
        )
    }

    fn draft(dispatch: u64) -> CapabilityMintDraft {
        CapabilityMintDraft::new(
            DispatchRequestId::new(dispatch),
            CapabilityScope::new(
                TicketId::new(dispatch),
                LaneId::new(1),
                CapabilityRole::Implementer,
                None,
            )
            .expect("the fixture scope binds"),
            McpOperations::new(["ticket.get", "timeline.query"])
                .expect("the fixture grant validates"),
            10,
        )
    }

    /// Seat one Ticket and one Lane, queue and claim one Dispatch
    /// Request for them, and answer the capability the claim minted.
    fn minted(database: &Database, dispatch: u64) -> Capability {
        let handle = database.connection_handle();
        let conn = handle.lock();
        conn.execute(
            "INSERT INTO tickets
                 (project_id, number, kind, priority, state, title, criteria,
                  subtype, mode, completion, version)
             VALUES (1, ?1, 'task', 'normal', 'draft', 'One slice', '[]',
                     'operational', 'human', '[\"done\"]', 1)",
            rusqlite::params![dispatch as i64],
        )
        .expect("the fixture Ticket lands");
        conn.execute(
            "INSERT INTO lanes (project_id, ticket_id, version) VALUES (1, ?1, 1)",
            rusqlite::params![dispatch as i64],
        )
        .expect("the fixture Lane lands");
        drop(conn);
        let store = SqliteDispatchStore::new(database);
        let created = store
            .enqueue(
                &kanban_app::DispatchEnqueue {
                    project: kanban_domain::ProjectId::new(1),
                    ticket: kanban_domain::TicketId::new(dispatch),
                    priority: kanban_domain::Priority::Normal,
                    ready: true,
                    harness: "claude-code".to_owned(),
                    model: "opus".to_owned(),
                    usage_pool: "operator".to_owned(),
                    created_at: 10,
                },
                &|id| {
                    TimelineEnvelope::project(
                        1,
                        TimelineEventKind::Transition,
                        Some(TimelineEntityRef {
                            kind: TimelineEntityKind::Ticket,
                            id: dispatch.to_string(),
                        }),
                        json!({ "action": "requested", "dispatch_request_id": id.value() }),
                    )
                },
            )
            .expect("the fixture request enqueues")
            .id();
        let (_, decision, capability) = store
            .try_claim(
                created,
                &kanban_app::ClaimContext {
                    defaults: kanban_domain::GlobalCapacity::restore(8, 8, 8),
                    project_caps: None,
                    active_lanes: 0,
                },
                envelope(CapabilityId::new(0), "claimed"),
                &|| Ok(draft(dispatch)),
                &|_mint, capability| envelope(capability, "capability_minted"),
            )
            .expect("the claim runs");
        assert!(matches!(decision, kanban_domain::ClaimDecision::Claim));
        capability.expect("a won claim mints")
    }

    #[test]
    fn a_capability_round_trips_and_survives_reopen() {
        let (dir, database) = database();
        let minted = minted(&database, 1);
        drop(database);

        let database =
            Database::open(&dir.path().join("kanban.sqlite")).expect("the database reopens");
        let restored = SqliteCapabilityStore::new(&database)
            .find(minted.id())
            .expect("the reload serves")
            .expect("the capability is durable");
        assert_eq!(restored, minted);
        assert_eq!(restored.status(), CapabilityStatus::Active);
        assert_eq!(restored.scope().role(), CapabilityRole::Implementer);
        assert_eq!(
            restored.operations().iter().collect::<Vec<_>>(),
            vec!["ticket.get", "timeline.query"]
        );
    }

    #[test]
    fn settlement_expires_durably_and_settling_again_changes_nothing() {
        let (dir, database) = database();
        let minted = minted(&database, 1);
        let store = SqliteCapabilityStore::new(&database);

        let settled = store
            .settle(
                minted.id(),
                200,
                envelope(minted.id(), "capability_settled"),
            )
            .expect("run settlement expires the capability");
        assert_eq!(settled.status(), CapabilityStatus::Settled);

        let again = store
            .settle(
                minted.id(),
                300,
                envelope(minted.id(), "capability_settled"),
            )
            .expect("settling twice is the same settlement");
        assert_eq!(again.status(), CapabilityStatus::Settled);
        assert_eq!(
            again, settled,
            "the first settlement's moment is the one that stands"
        );
        drop(database);

        let database =
            Database::open(&dir.path().join("kanban.sqlite")).expect("the database reopens");
        let restored = SqliteCapabilityStore::new(&database)
            .find(minted.id())
            .expect("the reload serves")
            .expect("the capability is durable");
        assert_eq!(restored.status(), CapabilityStatus::Settled);
    }

    #[test]
    fn a_second_capability_for_one_dispatch_is_refused() {
        let (_dir, database) = database();
        let minted = minted(&database, 1);

        let conn = database.connection();
        let span = WriteSpan::begin(&conn).expect("the span opens");
        let outcome = super::insert_capability(&span, &draft(1));
        drop(outcome);
        span.commit().expect("the span lands");

        assert_eq!(minted.dispatch().value(), 1);
        let rows: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM capabilities WHERE dispatch_request_id = 1",
                [],
                |row| row.get(0),
            )
            .expect("the count serves");
        assert_eq!(rows, 1, "one dispatch keeps exactly one capability");
    }

    #[test]
    fn settling_an_unknown_capability_is_not_found() {
        let (_dir, database) = database();

        let error = SqliteCapabilityStore::new(&database)
            .settle(
                CapabilityId::new(99),
                200,
                envelope(CapabilityId::new(99), "capability_settled"),
            )
            .expect_err("no such capability exists");

        assert_eq!(error.code, kanban_dto::ErrorCode::NotFound);
    }
}
