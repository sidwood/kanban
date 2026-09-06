//! SQLite runs (KAN-S9): durable rows freezing the requested and
//! effective profile snapshots at mint, one executing run per claimed
//! Dispatch Request (DR-EP-04, DR-EP-05).

use kanban_app::{RunMint, RunStore, TimelineEnvelope};
use kanban_domain::{DispatchRequestId, ProfileSnapshot, ProjectId, Run, RunId, RunStatus};
use kanban_dto::ApiError;
use rusqlite::params;

use crate::db::{ConnectionHandle, Database, WriteSpan};
use crate::timeline::insert_event;

const RUN_COLUMNS: &str = "id, project_id, ticket_id, dispatch_request_id, status,
                          requested_name, requested_harness, requested_model,
                          requested_effort, requested_usage_pool,
                          effective_name, effective_harness, effective_model,
                          effective_effort, effective_usage_pool,
                          fallback, fallback_path, created_at, version";

/// SQLite-backed runs.
pub struct SqliteRunStore {
    conn: ConnectionHandle,
}

impl SqliteRunStore {
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

impl RunStore for SqliteRunStore {
    fn mint(
        &self,
        draft: &RunMint,
        envelope: &dyn Fn(RunId) -> TimelineEnvelope,
    ) -> Result<Run, ApiError> {
        let fallback = draft.effective.name() != draft.requested.name();
        let fallback_path = if fallback {
            serde_json::to_string(&draft.fallback_path)
                .map_err(|error| ApiError::internal(&error.to_string()))?
        } else {
            "[]".to_owned()
        };
        let conn = self.lock();
        let span = WriteSpan::begin(&conn).map_err(internal)?;
        if executing_for_request(&span, draft.request.id())?.is_some() {
            return Err(ApiError::invalid_request(&format!(
                "Dispatch Request {} already holds an executing run",
                draft.request.id().value()
            )));
        }
        let outcome = span.execute(
            "INSERT INTO runs
                 (project_id, ticket_id, dispatch_request_id, status,
                  requested_name, requested_harness, requested_model,
                  requested_effort, requested_usage_pool,
                  effective_name, effective_harness, effective_model,
                  effective_effort, effective_usage_pool,
                  fallback, fallback_path, created_at, version)
             VALUES (?1, ?2, ?3, 'executing', ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13,
                     ?14, ?15, ?16, 1)",
            params![
                draft.request.project().value() as i64,
                draft.request.ticket().value() as i64,
                draft.request.id().value() as i64,
                draft.requested.name(),
                draft.requested.harness(),
                draft.requested.model(),
                draft.requested.effort(),
                draft.requested.usage_pool(),
                draft.effective.name(),
                draft.effective.harness(),
                draft.effective.model(),
                draft.effective.effort(),
                draft.effective.usage_pool(),
                if fallback { 1 } else { 0 },
                fallback_path,
                draft.created_at as i64,
            ],
        );
        match outcome {
            Ok(_) => {}
            Err(error) if is_executing_request_conflict(&error) => {
                return Err(ApiError::invalid_request(&format!(
                    "Dispatch Request {} already holds an executing run",
                    draft.request.id().value()
                )));
            }
            Err(error) => return Err(internal(error)),
        }
        let id = RunId::new(
            span.last_insert_rowid()
                .try_into()
                .map_err(|_| ApiError::internal("the run identity overflowed"))?,
        );
        let run = kanban_domain::Run::acknowledge(
            id,
            &draft.request,
            draft.requested.clone(),
            draft.effective.clone(),
            draft.fallback_path.clone(),
            draft.created_at,
        )
        .map_err(|error| ApiError::invalid_request(&error.to_string()))?;
        insert_event(&span, &envelope(run.id())).map_err(internal)?;
        span.commit().map_err(internal)?;
        Ok(run)
    }

    fn list_for_project(&self, project: ProjectId) -> Result<Vec<Run>, ApiError> {
        let conn = self.lock();
        let mut statement = conn
            .prepare(&format!(
                "SELECT {RUN_COLUMNS} FROM runs
                 WHERE project_id = ?1 ORDER BY created_at, id"
            ))
            .map_err(internal)?;
        let rows = statement
            .query_map(params![project.value() as i64], decode_run)
            .map_err(internal)?;
        let mut runs = Vec::new();
        for row in rows {
            runs.push(row.map_err(internal)?);
        }
        Ok(runs)
    }

    fn executing_for_request(&self, request: DispatchRequestId) -> Result<Option<Run>, ApiError> {
        let conn = self.lock();
        executing_for_request(&conn, request)
    }
}

fn executing_for_request(
    conn: &rusqlite::Connection,
    request: DispatchRequestId,
) -> Result<Option<Run>, ApiError> {
    match conn.query_row(
        &format!(
            "SELECT {RUN_COLUMNS} FROM runs
             WHERE dispatch_request_id = ?1 AND status = 'executing'"
        ),
        params![request.value() as i64],
        decode_run,
    ) {
        Ok(run) => Ok(Some(run)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(error) => Err(internal(error)),
    }
}

fn decode_run(row: &rusqlite::Row<'_>) -> rusqlite::Result<Run> {
    let status = RunStatus::parse(&row.get::<_, String>(4)?).ok_or_else(|| {
        rusqlite::Error::InvalidColumnType(4, "status".to_owned(), rusqlite::types::Type::Text)
    })?;
    let fallback_path: String = row.get(16)?;
    let fallback_path = serde_json::from_str(&fallback_path).unwrap_or_default();
    Ok(Run::restore(
        RunId::new(row.get::<_, i64>(0)? as u64),
        DispatchRequestId::new(row.get::<_, i64>(3)? as u64),
        ProjectId::new(row.get::<_, i64>(1)? as u64),
        kanban_domain::TicketId::new(row.get::<_, i64>(2)? as u64),
        status,
        ProfileSnapshot::restore(
            row.get(5)?,
            row.get(6)?,
            row.get(7)?,
            row.get(8)?,
            row.get(9)?,
        ),
        ProfileSnapshot::restore(
            row.get(10)?,
            row.get(11)?,
            row.get(12)?,
            row.get(13)?,
            row.get(14)?,
        ),
        row.get::<_, i64>(15)? != 0,
        fallback_path,
        row.get::<_, i64>(17)? as u64,
        row.get::<_, i64>(18)? as u64,
    ))
}

fn is_executing_request_conflict(error: &rusqlite::Error) -> bool {
    error
        .to_string()
        .contains("UNIQUE constraint failed: runs.dispatch_request_id")
}

fn internal(error: impl ToString) -> ApiError {
    ApiError::internal(&error.to_string())
}

#[cfg(test)]
mod tests {
    use super::SqliteRunStore;
    use crate::migrations::AllowAllMigrations;
    use crate::{Database, SqliteDispatchStore, SqliteProjectStore};
    use kanban_app::{
        ClaimContext, DispatchEnqueue, DispatchStore, ProjectStore, RunMint, RunStore,
    };
    use kanban_domain::{
        DispatchRequestId, GlobalCapacity, Priority, ProfileSnapshot, ProjectId,
        ProjectRegistration, RunStatus, TicketId,
    };
    use kanban_dto::{TimelineEntityKind, TimelineEntityRef, TimelineEventKind};
    use serde_json::json;

    fn database() -> (tempfile::TempDir, Database) {
        let dir = tempfile::tempdir().expect("a scratch directory is available");
        let mut database = Database::open(&dir.path().join("kanban.sqlite"))
            .expect("opening a fresh database succeeds");
        database
            .migrate(&AllowAllMigrations)
            .expect("the schema applies");
        seed(&database);
        (dir, database)
    }

    fn seed(database: &Database) {
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
                kanban_app::TimelineEnvelope::project(
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
    }

    fn envelope(run: u64) -> kanban_app::TimelineEnvelope {
        kanban_app::TimelineEnvelope::project(
            1,
            TimelineEventKind::Run,
            Some(TimelineEntityRef {
                kind: TimelineEntityKind::Run,
                id: run.to_string(),
            }),
            json!({ "action": "acknowledged", "run_id": run }),
        )
    }

    /// Queue, claim, and answer one Dispatch Request for Ticket 1.
    fn claimed_request(database: &Database) -> kanban_domain::DispatchRequest {
        let store = SqliteDispatchStore::new(database);
        let queued = store
            .enqueue(
                &DispatchEnqueue {
                    project: ProjectId::new(1),
                    ticket: TicketId::new(1),
                    priority: Priority::Normal,
                    ready: true,
                    harness: "claude-code".to_owned(),
                    model: "opus".to_owned(),
                    usage_pool: "operator".to_owned(),
                    created_at: 10,
                },
                &|id| envelope(id.value()),
            )
            .expect("the request enqueues");
        let roomy = ClaimContext {
            defaults: GlobalCapacity::restore(8, 8, 8),
            project_caps: None,
            active_lanes: 0,
        };
        store
            .try_claim(queued.id(), &roomy, envelope(0))
            .expect("the claim lands")
            .0
    }

    fn mint(request: &kanban_domain::DispatchRequest) -> RunMint {
        RunMint {
            request: request.clone(),
            requested: snapshot("nightly", "opus"),
            effective: snapshot("standard", "sonnet"),
            fallback_path: vec!["nightly".to_owned(), "standard".to_owned()],
            created_at: 20,
        }
    }

    fn snapshot(name: &str, model: &str) -> ProfileSnapshot {
        ProfileSnapshot::new(name, "claude-code", model, "high", "operator")
            .expect("a complete snapshot is accepted")
    }

    #[test]
    fn a_minted_run_round_trips_and_survives_reopen() {
        let (dir, database) = database();
        let request = claimed_request(&database);
        let minted = SqliteRunStore::new(&database)
            .mint(&mint(&request), &|id| envelope(id.value()))
            .expect("the run mints");
        assert_eq!(minted.status(), RunStatus::Executing);
        assert!(minted.fell_back());
        assert_eq!(minted.effective().name(), "standard");
        drop(database);

        let database =
            Database::open(&dir.path().join("kanban.sqlite")).expect("the database reopens");
        let restored = SqliteRunStore::new(&database)
            .executing_for_request(DispatchRequestId::new(minted.dispatch_request().value()))
            .expect("the reload serves")
            .expect("the run is durable");
        assert_eq!(restored, minted);
        assert_eq!(
            restored.fallback_path(),
            &["nightly".to_owned(), "standard".to_owned()]
        );
    }

    #[test]
    fn a_request_already_holding_an_executing_run_is_refused() {
        let (_dir, database) = database();
        let request = claimed_request(&database);
        let store = SqliteRunStore::new(&database);
        store
            .mint(&mint(&request), &|id| envelope(id.value()))
            .expect("the first run mints");

        let second = store.mint(&mint(&request), &|id| envelope(id.value()));

        let error = second.expect_err("one executing run per request");
        assert_eq!(error.code, kanban_dto::ErrorCode::InvalidRequest);
        assert!(
            error.message.contains("already"),
            "the refusal names the duplicate: {error:?}"
        );
    }

    #[test]
    fn an_unclaimed_request_mints_no_run() {
        let (_dir, database) = database();
        let store = SqliteDispatchStore::new(&database);
        let queued = store
            .enqueue(
                &DispatchEnqueue {
                    project: ProjectId::new(1),
                    ticket: TicketId::new(1),
                    priority: Priority::Normal,
                    ready: true,
                    harness: "claude-code".to_owned(),
                    model: "opus".to_owned(),
                    usage_pool: "operator".to_owned(),
                    created_at: 10,
                },
                &|id| envelope(id.value()),
            )
            .expect("the request enqueues");

        let error = SqliteRunStore::new(&database)
            .mint(&mint(&queued), &|id| envelope(id.value()))
            .expect_err("a queued request has no run");

        assert_eq!(error.code, kanban_dto::ErrorCode::InvalidRequest);
        assert!(
            error.message.contains("claimed"),
            "the refusal names the claim rule: {error:?}"
        );
    }

    #[test]
    fn the_project_listing_orders_runs_oldest_first() {
        let (_dir, database) = database();
        let request = claimed_request(&database);
        let store = SqliteRunStore::new(&database);
        store
            .mint(&mint(&request), &|id| envelope(id.value()))
            .expect("the run mints");
        // A second Ticket's request, minted later.
        database
            .connection()
            .execute(
                "INSERT INTO tickets
                     (project_id, number, kind, priority, state, title, criteria,
                      subtype, mode, completion, version)
                 VALUES (1, 2, 'task', 'normal', 'draft', 'Second slice', '[]',
                         'operational', 'human', '[\"done\"]', 1)",
                [],
            )
            .expect("the second Ticket lands");
        let dispatch = SqliteDispatchStore::new(&database);
        let queued = dispatch
            .enqueue(
                &DispatchEnqueue {
                    project: ProjectId::new(1),
                    ticket: TicketId::new(2),
                    priority: Priority::Normal,
                    ready: true,
                    harness: "claude-code".to_owned(),
                    model: "opus".to_owned(),
                    usage_pool: "operator".to_owned(),
                    created_at: 11,
                },
                &|id| envelope(id.value()),
            )
            .expect("the request enqueues");
        let roomy = ClaimContext {
            defaults: GlobalCapacity::restore(8, 8, 8),
            project_caps: None,
            active_lanes: 0,
        };
        let claimed = dispatch
            .try_claim(queued.id(), &roomy, envelope(0))
            .expect("the claim lands")
            .0;
        let later = RunMint {
            created_at: 30,
            ..mint(&claimed)
        };
        store
            .mint(&later, &|id| envelope(id.value()))
            .expect("the second run mints");

        let listed = store
            .list_for_project(ProjectId::new(1))
            .expect("the listing serves");

        let created: Vec<_> = listed.iter().map(|run| run.created_at()).collect();
        assert_eq!(created, vec![20, 30], "oldest first");
    }
}
