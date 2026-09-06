//! SQLite Dispatch Requests (KAN-S9): durable rows, and the claim
//! that evaluates capacity and updates status inside one write so
//! concurrent claimants see exactly one winner (DR-EP-08).

use kanban_app::{
    CapabilityMintDraft, ClaimContext, DispatchEnqueue, DispatchStore, TimelineEnvelope,
    evaluate_dispatch_claim,
};
use kanban_domain::{
    Capability, CapabilityId, ClaimDecision, DispatchError, DispatchRequest, DispatchRequestId,
    DispatchStatus, Priority, ProjectId, TicketId,
};
use kanban_dto::ApiError;
use rusqlite::params;

use crate::db::{ConnectionHandle, Database, WriteSpan};
use crate::timeline::insert_event;

const REQUEST_COLUMNS: &str = "id, project_id, ticket_id, status, priority, ready,
                               harness, model, usage_pool, created_at, version";

/// SQLite-backed Dispatch Requests.
pub struct SqliteDispatchStore {
    conn: ConnectionHandle,
}

impl SqliteDispatchStore {
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

impl DispatchStore for SqliteDispatchStore {
    fn enqueue(
        &self,
        draft: &DispatchEnqueue,
        envelope: &dyn Fn(DispatchRequestId) -> TimelineEnvelope,
    ) -> Result<DispatchRequest, ApiError> {
        let conn = self.lock();
        let span = WriteSpan::begin(&conn).map_err(internal)?;
        if open_status(&span, draft.ticket)?.is_some() {
            return Err(ApiError::invalid_request(
                &DispatchError::DuplicateOpen {
                    ticket: draft.ticket,
                }
                .to_string(),
            ));
        }
        let outcome = span.execute(
            "INSERT INTO dispatch_requests
                 (project_id, ticket_id, status, priority, ready,
                  harness, model, usage_pool, created_at, version)
             VALUES (?1, ?2, 'queued', ?3, ?4, ?5, ?6, ?7, ?8, 1)",
            params![
                draft.project.value() as i64,
                draft.ticket.value() as i64,
                draft.priority.wire_name(),
                if draft.ready { 1 } else { 0 },
                draft.harness,
                draft.model,
                draft.usage_pool,
                draft.created_at as i64,
            ],
        );
        match outcome {
            Ok(_) => {}
            Err(error) if is_open_ticket_conflict(&error) => {
                return Err(ApiError::invalid_request(
                    &DispatchError::DuplicateOpen {
                        ticket: draft.ticket,
                    }
                    .to_string(),
                ));
            }
            Err(error) => return Err(internal(error)),
        }
        let id = DispatchRequestId::new(
            span.last_insert_rowid()
                .try_into()
                .map_err(|_| ApiError::internal("the Dispatch Request identity overflowed"))?,
        );
        insert_event(&span, &envelope(id)).map_err(internal)?;
        span.commit().map_err(internal)?;
        DispatchRequest::enqueue(
            id,
            draft.project,
            draft.ticket,
            draft.priority,
            draft.ready,
            draft.harness.clone(),
            draft.model.clone(),
            draft.usage_pool.clone(),
            draft.created_at,
        )
        .map_err(|error| ApiError::invalid_request(&error.to_string()))
    }

    fn find(&self, id: DispatchRequestId) -> Result<Option<DispatchRequest>, ApiError> {
        let conn = self.lock();
        match conn.query_row(
            &format!("SELECT {REQUEST_COLUMNS} FROM dispatch_requests WHERE id = ?1"),
            params![id.value() as i64],
            decode_request,
        ) {
            Ok(request) => Ok(Some(request)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(error) => Err(internal(error)),
        }
    }

    fn open_for_ticket(&self, ticket: TicketId) -> Result<Option<DispatchStatus>, ApiError> {
        let conn = self.lock();
        open_status(&conn, ticket)
    }

    fn try_claim(
        &self,
        id: DispatchRequestId,
        context: &ClaimContext,
        envelope: TimelineEnvelope,
        mint: &dyn Fn() -> Result<CapabilityMintDraft, ApiError>,
        mint_envelope: &dyn Fn(&CapabilityMintDraft, CapabilityId) -> TimelineEnvelope,
    ) -> Result<(DispatchRequest, ClaimDecision, Option<Capability>), ApiError> {
        let conn = self.lock();
        let span = WriteSpan::begin(&conn).map_err(internal)?;
        let mut request = span
            .query_row(
                &format!("SELECT {REQUEST_COLUMNS} FROM dispatch_requests WHERE id = ?1"),
                params![id.value() as i64],
                decode_request,
            )
            .map_err(|error| match error {
                rusqlite::Error::QueryReturnedNoRows => {
                    ApiError::not_found(&format!("dispatch request {}", id.value()))
                }
                other => internal(other),
            })?;
        let claimed = listed_claimed(&span)?;
        let decision = evaluate_dispatch_claim(&request, &claimed, context);
        request
            .apply_claim(decision.clone())
            .map_err(|error| ApiError::invalid_request(&error.to_string()))?;
        let mut capability = None;
        if matches!(decision, ClaimDecision::Claim) {
            let changed = span
                .execute(
                    "UPDATE dispatch_requests
                     SET status = 'claimed', version = version + 1
                     WHERE id = ?1 AND status = 'queued'",
                    params![id.value() as i64],
                )
                .map_err(internal)?;
            if changed != 1 {
                return Err(ApiError::invalid_request(
                    "the Dispatch Request is already claimed",
                ));
            }
            insert_event(&span, &envelope).map_err(internal)?;
            // A mint failure discards the span, and with it the win:
            // a claim that cannot grant its authority is not a claim.
            let draft = mint()?;
            let minted = crate::capability::insert_capability(&span, &draft)?;
            insert_event(&span, &mint_envelope(&draft, minted.id())).map_err(internal)?;
            capability = Some(minted);
        }
        span.commit().map_err(internal)?;
        Ok((request, decision, capability))
    }

    fn list_queued(&self, project: ProjectId) -> Result<Vec<DispatchRequest>, ApiError> {
        let conn = self.lock();
        let mut statement = conn
            .prepare(&format!(
                "SELECT {REQUEST_COLUMNS} FROM dispatch_requests
                 WHERE project_id = ?1 AND status = 'queued'"
            ))
            .map_err(internal)?;
        let rows = statement
            .query_map(params![project.value() as i64], decode_request)
            .map_err(internal)?;
        let mut requests = Vec::new();
        for row in rows {
            requests.push(row.map_err(internal)?);
        }
        Ok(requests)
    }
}

fn listed_claimed(conn: &rusqlite::Connection) -> Result<Vec<DispatchRequest>, ApiError> {
    let mut statement = conn
        .prepare(&format!(
            "SELECT {REQUEST_COLUMNS} FROM dispatch_requests WHERE status = 'claimed'"
        ))
        .map_err(internal)?;
    let rows = statement.query_map([], decode_request).map_err(internal)?;
    let mut claimed = Vec::new();
    for row in rows {
        claimed.push(row.map_err(internal)?);
    }
    Ok(claimed)
}

fn open_status(
    conn: &rusqlite::Connection,
    ticket: TicketId,
) -> Result<Option<DispatchStatus>, ApiError> {
    match conn.query_row(
        "SELECT status FROM dispatch_requests
         WHERE ticket_id = ?1 AND status IN ('queued', 'claimed')",
        params![ticket.value() as i64],
        |row| row.get::<_, String>(0),
    ) {
        Ok(status) => DispatchStatus::parse(&status)
            .map(Some)
            .ok_or_else(|| ApiError::internal("a stored Dispatch Request status is unknown")),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(error) => Err(internal(error)),
    }
}

fn decode_request(row: &rusqlite::Row<'_>) -> rusqlite::Result<DispatchRequest> {
    let status = DispatchStatus::parse(&row.get::<_, String>(3)?).ok_or_else(|| {
        rusqlite::Error::InvalidColumnType(3, "status".to_owned(), rusqlite::types::Type::Text)
    })?;
    let priority = Priority::parse(&row.get::<_, String>(4)?).ok_or_else(|| {
        rusqlite::Error::InvalidColumnType(4, "priority".to_owned(), rusqlite::types::Type::Text)
    })?;
    Ok(DispatchRequest::restore(
        DispatchRequestId::new(row.get::<_, i64>(0)? as u64),
        ProjectId::new(row.get::<_, i64>(1)? as u64),
        TicketId::new(row.get::<_, i64>(2)? as u64),
        status,
        priority,
        row.get::<_, i64>(5)? != 0,
        row.get(6)?,
        row.get(7)?,
        row.get(8)?,
        row.get::<_, i64>(9)? as u64,
        row.get::<_, i64>(10)? as u64,
    ))
}

fn is_open_ticket_conflict(error: &rusqlite::Error) -> bool {
    error
        .to_string()
        .contains("UNIQUE constraint failed: dispatch_requests.ticket_id")
}

fn internal(error: impl ToString) -> ApiError {
    ApiError::internal(&error.to_string())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::thread;

    use kanban_app::{
        CapabilityMintDraft, ClaimContext, DispatchEnqueue, DispatchStore, ProjectStore,
    };
    use kanban_domain::{
        CapabilityId, CapabilityRole, CapabilityScope, ClaimDecision, DispatchRequestId,
        DispatchStatus, GlobalCapacity, LaneId, McpOperations, Priority, ProjectId,
        ProjectRegistration, TicketId,
    };
    use kanban_dto::{TimelineEntityKind, TimelineEntityRef, TimelineEventKind};
    use serde_json::json;

    use super::SqliteDispatchStore;
    use crate::migrations::AllowAllMigrations;
    use crate::{Database, SqliteProjectStore};
    use kanban_app::TimelineEnvelope;

    fn database() -> (tempfile::TempDir, Database) {
        let dir = tempfile::tempdir().expect("a scratch directory is available");
        let mut database = Database::open(&dir.path().join("kanban.sqlite"))
            .expect("opening a fresh database succeeds");
        database
            .migrate(&AllowAllMigrations)
            .expect("the schema applies");
        seed_project_and_tickets(&database, 8);
        (dir, database)
    }

    fn seed_project_and_tickets(database: &Database, tickets: u64) {
        let projects = SqliteProjectStore::new(database);
        let registration = ProjectRegistration::new(
            "CORE",
            "Control plane",
            "/repositories/kanban",
            "/workspaces/kanban.seed",
            "main",
            "kanban.seed",
            Some("kanban-main"),
            None,
        )
        .expect("the fixture registration validates");
        projects
            .create(&registration, &|id| {
                TimelineEnvelope::project(
                    id.value(),
                    TimelineEventKind::Transition,
                    Some(TimelineEntityRef {
                        kind: TimelineEntityKind::Project,
                        id: id.value().to_string(),
                    }),
                    json!({ "action": "registered" }),
                )
            })
            .expect("the fixture Project lands");
        for number in 1..=tickets {
            database
                .connection()
                .execute(
                    "INSERT INTO tickets
                         (project_id, number, kind, priority, state, title, criteria,
                          subtype, mode, completion, version)
                     VALUES (1, ?1, 'task', 'normal', 'draft', 'One slice', '[]',
                             'operational', 'human', '[\"done\"]', 1)",
                    rusqlite::params![number as i64],
                )
                .expect("the fixture Ticket lands");
        }
        database
            .connection()
            .execute("INSERT INTO lanes (project_id, version) VALUES (1, 1)", [])
            .expect("the fixture Lane lands");
    }

    fn envelope(ticket: u64, request: DispatchRequestId, action: &str) -> TimelineEnvelope {
        TimelineEnvelope::project(
            1,
            TimelineEventKind::Transition,
            Some(TimelineEntityRef {
                kind: TimelineEntityKind::Ticket,
                id: ticket.to_string(),
            }),
            json!({
                "action": action,
                "dispatch_request_id": request.value(),
            }),
        )
    }

    fn enqueue(store: &SqliteDispatchStore, ticket: u64, created_at: u64) -> DispatchRequestId {
        store
            .enqueue(
                &DispatchEnqueue {
                    project: ProjectId::new(1),
                    ticket: TicketId::new(ticket),
                    priority: Priority::Normal,
                    ready: true,
                    harness: "claude-code".to_owned(),
                    model: "opus".to_owned(),
                    usage_pool: "operator".to_owned(),
                    created_at,
                },
                &|id| envelope(ticket, id, "requested"),
            )
            .expect("the request enqueues")
            .id()
    }

    fn roomy() -> ClaimContext {
        ClaimContext {
            defaults: GlobalCapacity::restore(8, 8, 8),
            project_caps: None,
            active_lanes: 0,
        }
    }

    fn one_harness() -> ClaimContext {
        ClaimContext {
            defaults: GlobalCapacity::restore(1, 8, 8),
            project_caps: None,
            active_lanes: 0,
        }
    }

    /// The implementer mint a won claim carries, bound to the
    /// fixture's Ticket and a Lane holding it. Fixtures seat one
    /// Ticket per dispatch identity, so both share `claimant`.
    fn mint_draft(claimant: u64) -> CapabilityMintDraft {
        CapabilityMintDraft::new(
            DispatchRequestId::new(claimant),
            CapabilityScope::new(
                TicketId::new(claimant),
                LaneId::new(1),
                CapabilityRole::Implementer,
                None,
            )
            .expect("the fixture scope binds"),
            McpOperations::new(["ticket.get"]).expect("the fixture grant validates"),
            10,
        )
    }

    /// The timeline row a minted capability leaves.
    fn mint_envelope(_mint: &CapabilityMintDraft, capability: CapabilityId) -> TimelineEnvelope {
        TimelineEnvelope::project(
            1,
            TimelineEventKind::Transition,
            Some(TimelineEntityRef {
                kind: TimelineEntityKind::Ticket,
                id: "1".to_owned(),
            }),
            json!({ "action": "capability_minted", "capability_id": capability.value() }),
        )
    }

    #[test]
    fn a_request_round_trips_and_survives_reopen() {
        let (dir, database) = database();
        let id = enqueue(&SqliteDispatchStore::new(&database), 1, 10);
        drop(database);

        let database =
            Database::open(&dir.path().join("kanban.sqlite")).expect("the database reopens");
        let restored = SqliteDispatchStore::new(&database)
            .find(id)
            .expect("the reload serves")
            .expect("the request is durable");
        assert_eq!(restored.status(), DispatchStatus::Queued);
        assert_eq!(restored.ticket(), TicketId::new(1));
        assert_eq!(restored.harness(), "claude-code");
    }

    #[test]
    fn concurrent_claimants_of_one_request_produce_exactly_one_winner() {
        let (_dir, database) = database();
        let store = Arc::new(SqliteDispatchStore::new(&database));
        let id = enqueue(&store, 1, 10);

        let mut joins = Vec::new();
        for _ in 0..8 {
            let store = store.clone();
            joins.push(thread::spawn(move || {
                store.try_claim(
                    id,
                    &roomy(),
                    envelope(1, id, "claimed"),
                    &|| Ok(mint_draft(1)),
                    &mint_envelope,
                )
            }));
        }
        let mut wins = 0;
        let mut already = 0;
        for join in joins {
            match join.join().expect("the claimant finishes") {
                Ok((_, ClaimDecision::Claim, _)) => wins += 1,
                Ok((_, ClaimDecision::AlreadyClaimed, _)) => already += 1,
                Ok((_, other, _)) => panic!("unexpected decision {other:?}"),
                Err(error) => {
                    assert!(
                        error.message.contains("already claimed"),
                        "the only refused race is an already-claimed request, got {error:?}"
                    );
                    already += 1;
                }
            }
        }
        assert_eq!(wins, 1, "exactly one concurrent claimant wins");
        assert_eq!(already, 7, "the other seven lose the race");
        let restored = store.find(id).expect("the reload serves").expect("durable");
        assert_eq!(restored.status(), DispatchStatus::Claimed);
    }

    #[test]
    fn concurrent_claimants_without_capacity_leave_losers_queued() {
        let (_dir, database) = database();
        let store = Arc::new(SqliteDispatchStore::new(&database));
        let ids: Vec<_> = (1..=8)
            .map(|ticket| enqueue(&store, ticket, ticket))
            .collect();

        let mut joins = Vec::new();
        for id in ids.clone() {
            let store = store.clone();
            joins.push(thread::spawn(move || {
                store.try_claim(
                    id,
                    &one_harness(),
                    envelope(id.value(), id, "claimed"),
                    &|| Ok(mint_draft(id.value())),
                    &mint_envelope,
                )
            }));
        }
        let mut wins = 0;
        let mut queued = 0;
        for join in joins {
            match join.join().expect("the claimant finishes") {
                Ok((_, ClaimDecision::Claim, _)) => wins += 1,
                Ok((_, ClaimDecision::RemainQueued(_), _)) => queued += 1,
                other => panic!("unexpected outcome {other:?}"),
            }
        }
        assert_eq!(wins, 1, "capacity of one harness admits one winner");
        assert_eq!(queued, 7, "losers remain queued");
        let queued_now = store
            .list_queued(ProjectId::new(1))
            .expect("the queue serves");
        assert_eq!(queued_now.len(), 7);
    }
}
