//! The SQLite implementation of the Lane storage port.

use kanban_app::{LaneStore, TimelineEnvelope};
use kanban_domain::{Lane, LaneId, ProjectId, TicketId, Workspace, WorkspaceId};
use kanban_dto::ApiError;
use rusqlite::params;

use crate::db::{ConnectionHandle, Database, WriteSpan};
use crate::timeline::insert_event;

const LANE_COLUMNS: &str = "id, project_id, workspace_id, ticket_id, version";

/// The Lane port over the authoritative database.
pub struct SqliteLaneStore {
    conn: ConnectionHandle,
}

impl SqliteLaneStore {
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

impl LaneStore for SqliteLaneStore {
    fn create(
        &self,
        project_id: ProjectId,
        envelope: &dyn Fn(LaneId) -> TimelineEnvelope,
    ) -> Result<Lane, ApiError> {
        let conn = self.lock();
        let span = WriteSpan::begin(&conn).map_err(internal)?;
        span.execute(
            "INSERT INTO lanes (project_id, workspace_id, ticket_id, version)
             VALUES (?1, NULL, NULL, 1)",
            params![project_id.value() as i64],
        )
        .map_err(internal)?;
        let id = LaneId::new(
            span.last_insert_rowid()
                .try_into()
                .map_err(|_| ApiError::internal("the Lane identity overflowed"))?,
        );
        insert_event(&span, &envelope(id)).map_err(internal)?;
        span.commit().map_err(internal)?;
        Ok(Lane::new(id, project_id))
    }

    fn find(&self, id: LaneId) -> Result<Option<Lane>, ApiError> {
        let conn = self.lock();
        let row = conn.query_row(
            &format!("SELECT {LANE_COLUMNS} FROM lanes WHERE id = ?1"),
            params![id.value() as i64],
            decode_row,
        );
        match row {
            Ok(lane) => Ok(Some(lane)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(error) => Err(internal(error)),
        }
    }

    fn list_for_project(&self, project_id: ProjectId) -> Result<Vec<Lane>, ApiError> {
        let conn = self.lock();
        let mut statement = conn
            .prepare(&format!(
                "SELECT {LANE_COLUMNS}
                 FROM lanes
                 WHERE project_id = ?1
                 ORDER BY id"
            ))
            .map_err(internal)?;
        let rows = statement
            .query_map(params![project_id.value() as i64], decode_row)
            .map_err(internal)?;
        let mut lanes = Vec::new();
        for row in rows {
            lanes.push(row.map_err(internal)?);
        }
        Ok(lanes)
    }

    fn find_by_workspace(
        &self,
        project_id: ProjectId,
        workspace_id: WorkspaceId,
    ) -> Result<Option<Lane>, ApiError> {
        let conn = self.lock();
        let row = conn.query_row(
            &format!(
                "SELECT {LANE_COLUMNS}
                 FROM lanes
                 WHERE project_id = ?1 AND workspace_id = ?2"
            ),
            params![project_id.value() as i64, workspace_id.value() as i64],
            decode_row,
        );
        match row {
            Ok(lane) => Ok(Some(lane)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(error) => Err(internal(error)),
        }
    }

    fn save(&self, lane: &Lane, envelope: TimelineEnvelope) -> Result<(), ApiError> {
        let conn = self.lock();
        let span = WriteSpan::begin(&conn).map_err(internal)?;
        update_lane(&span, lane)?;
        insert_event(&span, &envelope).map_err(internal)?;
        span.commit().map_err(internal)?;
        Ok(())
    }

    fn save_with_workspace(
        &self,
        lane: &Lane,
        lane_envelope: TimelineEnvelope,
        workspace: &Workspace,
        workspace_envelope: TimelineEnvelope,
    ) -> Result<(), ApiError> {
        let conn = self.lock();
        let span = WriteSpan::begin(&conn).map_err(internal)?;
        update_lane(&span, lane)?;
        // The mirror follows the claim it was handed: the Lane write
        // carries the optimistic guard, and the Workspace row moves
        // with it in the same span, claim pointer, health, and all.
        span.execute(
            "UPDATE workspaces
             SET lane_id = ?2,
                 health = ?3,
                 version = ?4
             WHERE id = ?1",
            params![
                workspace.id().value() as i64,
                workspace.lane_id().map(|id| id as i64),
                workspace.health().as_str(),
                workspace.version() as i64,
            ],
        )
        .map_err(internal)?;
        insert_event(&span, &lane_envelope).map_err(internal)?;
        insert_event(&span, &workspace_envelope).map_err(internal)?;
        span.commit().map_err(internal)?;
        Ok(())
    }

    fn record_refusal(&self, envelope: TimelineEnvelope) -> Result<(), ApiError> {
        let conn = self.lock();
        // Called with no enclosing command span, this is its own
        // transaction: the refusal commits alone, after the refused
        // command's span has already rolled back (DR-LW-07).
        let span = WriteSpan::begin(&conn).map_err(internal)?;
        insert_event(&span, &envelope).map_err(internal)?;
        span.commit().map_err(internal)?;
        Ok(())
    }
}

/// Land one Lane row under its optimistic version guard, translating
/// the row-level uniqueness of a claim into the stable conflict
/// refusal (DR-LW-02, DR-LW-03).
fn update_lane(span: &WriteSpan<'_>, lane: &Lane) -> Result<(), ApiError> {
    let preceding_version = lane.version() - 1;
    let changed = match span.execute(
        "UPDATE lanes
         SET workspace_id = ?2,
             ticket_id = ?3,
             version = ?4
         WHERE id = ?1 AND version = ?5",
        params![
            lane.id().value() as i64,
            lane.workspace_id()
                .map(WorkspaceId::value)
                .map(|id| id as i64),
            lane.ticket_id().map(TicketId::value).map(|id| id as i64),
            lane.version() as i64,
            preceding_version as i64,
        ],
    ) {
        Ok(changed) => changed,
        Err(error) => {
            let message = error.to_string();
            if message.contains("UNIQUE constraint failed: lanes.workspace_id") {
                return Err(workspace_claim_conflict());
            }
            if message.contains("UNIQUE constraint failed: lanes.ticket_id") {
                return Err(ticket_slot_conflict());
            }
            return Err(internal(error));
        }
    };
    if changed != 1 {
        let current = span.query_row(
            "SELECT version FROM lanes WHERE id = ?1",
            params![lane.id().value() as i64],
            |row| row.get::<_, i64>(0),
        );
        return match current {
            Ok(current) => Err(ApiError::stale_version(
                preceding_version,
                current.unsigned_abs(),
            )),
            Err(rusqlite::Error::QueryReturnedNoRows) => {
                Err(ApiError::not_found(&format!("lane {}", lane.id())))
            }
            Err(error) => Err(internal(error)),
        };
    }
    Ok(())
}

/// The stable refusal for a Workspace another Lane already claims.
pub fn workspace_claim_conflict() -> ApiError {
    ApiError::invalid_request("the Workspace already belongs to another Lane")
}

/// The stable refusal for a Ticket another Lane already holds.
pub fn ticket_slot_conflict() -> ApiError {
    ApiError::invalid_request("the Ticket is already held by another Lane")
}

fn decode_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Lane> {
    Ok(Lane::restore(
        LaneId::new(row.get::<_, i64>(0)? as u64),
        ProjectId::new(row.get::<_, i64>(1)? as u64),
        row.get::<_, Option<i64>>(2)?
            .map(|id| WorkspaceId::new(id as u64)),
        row.get::<_, Option<i64>>(3)?
            .map(|id| TicketId::new(id as u64)),
        row.get::<_, i64>(4)? as u64,
    ))
}

fn internal(error: impl std::error::Error) -> ApiError {
    ApiError::internal(&error.to_string())
}

#[cfg(test)]
mod lane_store {
    use kanban_app::{LaneStore, TimelineEnvelope, WorkspaceStore};
    use kanban_domain::{
        Lane, ProjectId, TicketId, Workspace, WorkspaceCheckout, WorkspaceHealth, WorkspaceId,
        WorkspaceRegistration,
    };
    use kanban_dto::{TimelineEntityKind, TimelineEntityRef, TimelineEventKind};
    use rusqlite::params;
    use serde_json::json;

    use super::SqliteLaneStore;
    use crate::db::Database;
    use crate::migrations::AllowAllMigrations;
    use crate::test_support::scratch_database;
    use crate::workspaces::SqliteWorkspaceStore;

    fn store() -> (tempfile::TempDir, Database, SqliteLaneStore) {
        let (dir, mut database) = scratch_database();
        database
            .migrate(&AllowAllMigrations)
            .expect("the migrations apply");
        database
            .connection()
            .execute(
                "INSERT INTO projects
                     (code, name, repository, seed_workspace, default_branch,
                      herdr_workspace, herdr_session, archived, version)
                 VALUES ('CORE', 'Control plane', '/repositories/kanban',
                         '/workspaces/kanban.seed', 'main', 'kanban.seed', 'kanban-main', 0, 1)",
                [],
            )
            .expect("the fixture Project lands");
        let store = SqliteLaneStore::new(&database);
        (dir, database, store)
    }

    fn lane_envelope(lane: &Lane, action: &str) -> TimelineEnvelope {
        TimelineEnvelope::project(
            1,
            TimelineEventKind::Transition,
            Some(TimelineEntityRef {
                kind: TimelineEntityKind::Lane,
                id: lane.id().value().to_string(),
            }),
            json!({ "action": action, "id": lane.id().value() }),
        )
    }

    fn workspace_envelope(workspace: &Workspace, action: &str) -> TimelineEnvelope {
        TimelineEnvelope::project(
            1,
            TimelineEventKind::Transition,
            Some(TimelineEntityRef {
                kind: TimelineEntityKind::Workspace,
                id: workspace.id().value().to_string(),
            }),
            json!({ "action": action, "id": workspace.id().value() }),
        )
    }

    fn registered_workspace(database: &Database) -> Workspace {
        let workspaces = SqliteWorkspaceStore::new(database);
        let registration =
            WorkspaceRegistration::new(ProjectId::new(1), "/workspaces/kanban.feature", false)
                .expect("the fixture registration validates");
        let mut workspace = workspaces
            .create(&registration, &|id| {
                TimelineEnvelope::project(
                    1,
                    TimelineEventKind::Transition,
                    Some(TimelineEntityRef {
                        kind: TimelineEntityKind::Workspace,
                        id: id.value().to_string(),
                    }),
                    json!({ "action": "registered", "id": id.value() }),
                )
            })
            .expect("the workspace registers");
        workspace
            .observe(
                true,
                Some("identity".to_owned()),
                Some(WorkspaceCheckout::Branch("feature".to_owned())),
                Some("abc123".to_owned()),
                Some(true),
                Some(false),
            )
            .expect("the observation applies");
        workspaces
            .save(&workspace, workspace_envelope(&workspace, "observed"))
            .expect("the observation persists");
        workspace
    }

    /// Insert one Task Ticket row, standing in for a Ticket the
    /// Project minted before the fixture began.
    fn stored_ticket(database: &Database) {
        database
            .connection()
            .execute(
                "INSERT INTO tickets
                     (project_id, number, kind, priority, state, title, criteria, version)
                 VALUES (1, 1, 'task', 'normal', 'draft', 'One slice', '[]', 1)",
                [],
            )
            .expect("the fixture Ticket lands");
    }

    #[test]
    fn a_lane_round_trips_its_claims() {
        let (_dir, database, store) = store();
        stored_ticket(&database);
        let workspace = registered_workspace(&database);
        let lane = store
            .create(ProjectId::new(1), &|id| {
                lane_envelope(&Lane::new(id, ProjectId::new(1)), "created")
            })
            .expect("the lane creates");

        let mut lane = lane;
        lane.assign_workspace(&workspace)
            .expect("the claim applies");
        store
            .save(&lane, lane_envelope(&lane, "workspace_assigned"))
            .expect("the workspace claim persists");
        lane.assign_ticket(TicketId::new(1))
            .expect("the ticket holds");
        store
            .save(&lane, lane_envelope(&lane, "ticket_assigned"))
            .expect("the ticket claim persists");

        let restored = store
            .find(lane.id())
            .expect("the lane loads")
            .expect("the lane exists");
        assert_eq!(restored.workspace_id(), Some(workspace.id()));
        assert_eq!(restored.ticket_id(), Some(TicketId::new(1)));
        assert_eq!(restored.version(), 3);
        assert_eq!(restored.project(), ProjectId::new(1));
    }

    #[test]
    fn the_workspace_mirror_lands_in_the_same_write() {
        let (dir, database, store) = store();
        let workspace = registered_workspace(&database);
        let workspaces = SqliteWorkspaceStore::new(&database);
        let lane = store
            .create(ProjectId::new(1), &|id| {
                lane_envelope(&Lane::new(id, ProjectId::new(1)), "created")
            })
            .expect("the lane creates");
        let mut lane = lane;
        lane.assign_workspace(&workspace)
            .expect("the claim applies");
        let mut workspace = workspace;
        workspace.assign_lane(lane.id().value());

        store
            .save_with_workspace(
                &lane,
                lane_envelope(&lane, "workspace_assigned"),
                &workspace,
                workspace_envelope(&workspace, "lane_assigned"),
            )
            .expect("the pair persists");

        let stored = workspaces
            .find(workspace.id())
            .expect("the workspace loads")
            .expect("the workspace exists");
        assert_eq!(stored.lane_id(), Some(lane.id().value()));
        assert_eq!(stored.health(), WorkspaceHealth::Assigned);
        let _ = dir;
    }

    #[test]
    fn a_workspace_cannot_be_claimed_by_two_lanes_at_the_row_level() {
        let (_dir, database, store) = store();
        let workspace = registered_workspace(&database);
        let first = store
            .create(ProjectId::new(1), &|id| {
                lane_envelope(&Lane::new(id, ProjectId::new(1)), "created")
            })
            .expect("the first lane creates");
        let second = store
            .create(ProjectId::new(1), &|id| {
                lane_envelope(&Lane::new(id, ProjectId::new(1)), "created")
            })
            .expect("the second lane creates");

        let mut first = first;
        first
            .assign_workspace(&workspace)
            .expect("the claim applies");
        store
            .save(&first, lane_envelope(&first, "workspace_assigned"))
            .expect("the first claim persists");

        // A racing writer that never saw the first claim: the
        // row-level UNIQUE guard refuses what the aggregate could
        // not see (DR-LW-03).
        let racing = Lane::restore(
            second.id(),
            ProjectId::new(1),
            Some(workspace.id()),
            None,
            2,
        );
        let error = store
            .save(&racing, lane_envelope(&racing, "workspace_assigned"))
            .expect_err("the row-level guard refuses the second claim");

        assert!(
            error.message.contains("belongs to another Lane"),
            "the refusal is the stable conflict: {}",
            error.message
        );
        let holder = store
            .find_by_workspace(ProjectId::new(1), workspace.id())
            .expect("the lookup serves")
            .expect("the original holder stands");
        assert_eq!(holder.id(), first.id());
    }

    #[test]
    fn a_refusal_row_lands_without_changing_lanes() {
        let (_dir, database, store) = store();
        let lane = store
            .create(ProjectId::new(1), &|id| {
                lane_envelope(&Lane::new(id, ProjectId::new(1)), "created")
            })
            .expect("the lane creates");

        store
            .record_refusal(TimelineEnvelope::project(
                1,
                TimelineEventKind::Transition,
                Some(TimelineEntityRef {
                    kind: TimelineEntityKind::Lane,
                    id: lane.id().value().to_string(),
                }),
                json!({
                    "action": "seed_assignment_refused",
                    "id": lane.id().value(),
                    "path": "/workspaces/kanban.seed",
                    "reason": "seed",
                }),
            ))
            .expect("the refusal records");

        let rows: i64 = database
            .connection()
            .query_row(
                "SELECT COUNT(*) FROM timeline_events
                 WHERE entity_kind = 'lane'
                   AND json_extract(detail, '$.action') = 'seed_assignment_refused'",
                [],
                |row| row.get(0),
            )
            .expect("the timeline is readable");
        assert_eq!(rows, 1, "the refusal row is durable");
        let lanes = store
            .list_for_project(ProjectId::new(1))
            .expect("the listing serves");
        assert_eq!(lanes.len(), 1);
        assert_eq!(lanes[0].version(), 1, "the refusal changed no aggregate");
    }

    #[test]
    fn saving_with_a_stale_lane_version_is_refused() {
        let (_dir, database, store) = store();
        stored_ticket(&database);
        let lane = store
            .create(ProjectId::new(1), &|id| {
                lane_envelope(&Lane::new(id, ProjectId::new(1)), "created")
            })
            .expect("the lane creates");

        let mut lane = lane;
        lane.assign_ticket(TicketId::new(1))
            .expect("the ticket holds");
        store
            .save(&lane, lane_envelope(&lane, "ticket_assigned"))
            .expect("the write lands at version two");

        // A writer still holding the pre-assignment state.
        let stale = Lane::restore(lane.id(), ProjectId::new(1), None, None, 1);
        let error = store
            .save(&stale, lane_envelope(&stale, "created"))
            .expect_err("the stale write is refused");

        assert_eq!(error.code, kanban_dto::ErrorCode::StaleVersion);
        assert_eq!(error.current_version, Some(2));
    }

    #[test]
    fn deleting_a_lane_is_refused_by_the_database() {
        let (_dir, database, store) = store();
        let lane = store
            .create(ProjectId::new(1), &|id| {
                lane_envelope(&Lane::new(id, ProjectId::new(1)), "created")
            })
            .expect("the lane creates");

        let error = database
            .connection()
            .execute(
                "DELETE FROM lanes WHERE id = ?1",
                params![lane.id().value() as i64],
            )
            .expect_err("the trigger refuses deletion");

        assert!(
            error.to_string().contains("never deleted"),
            "the refusal names the rule: {error}"
        );
    }

    #[test]
    fn finding_by_workspace_reports_the_claiming_lane() {
        let (_dir, _database, store) = store();
        let lane = store
            .create(ProjectId::new(1), &|id| {
                lane_envelope(&Lane::new(id, ProjectId::new(1)), "created")
            })
            .expect("the lane creates");
        let mut lane = lane;
        lane.assign_workspace(&registered_workspace(&_database))
            .expect("the claim applies");
        store
            .save(&lane, lane_envelope(&lane, "workspace_assigned"))
            .expect("the claim persists");

        let holder = store
            .find_by_workspace(ProjectId::new(1), WorkspaceId::new(1))
            .expect("the lookup serves")
            .expect("the claiming lane is found");
        assert_eq!(holder.id(), lane.id());
        let absent = store
            .find_by_workspace(ProjectId::new(1), WorkspaceId::new(9))
            .expect("the lookup serves");
        assert!(absent.is_none());
    }
}
