//! The SQLite implementation of the guarded clone commands' timeline
//! port (KAN-S6-US4). The rows are the durable record of every
//! guarded invocation and every refusal.

use kanban_app::{CloneGuardStore, TimelineEnvelope};
use kanban_dto::ApiError;

use crate::db::{ConnectionHandle, Database, WriteSpan};
use crate::timeline::insert_event;

/// The clone guard timeline port over the authoritative database.
pub struct SqliteCloneGuardStore {
    conn: ConnectionHandle,
}

impl SqliteCloneGuardStore {
    /// Share the connection the `database` owns.
    pub fn new(database: &Database) -> Self {
        Self {
            conn: database.connection_handle(),
        }
    }
}

impl CloneGuardStore for SqliteCloneGuardStore {
    fn append(&self, envelope: TimelineEnvelope) -> Result<(), ApiError> {
        let conn = self.conn.lock();
        // A write span nests inside the command's mutation span when
        // one is open, so an invocation row commits with its command;
        // after a discard the span is the row's own transaction, so a
        // refusal commits alone, after the rejected command has rolled
        // back (DR-LW-09, DR-LW-10).
        let span = WriteSpan::begin(&conn).map_err(internal)?;
        insert_event(&span, &envelope).map_err(internal)?;
        span.commit().map_err(internal)?;
        Ok(())
    }
}

fn internal(error: impl std::error::Error) -> ApiError {
    ApiError::internal(&error.to_string())
}

#[cfg(test)]
mod clone_guard_store {
    use kanban_dto::{TimelineEntityKind, TimelineEntityRef, TimelineEventKind};

    use super::SqliteCloneGuardStore;
    use crate::migrations::AllowAllMigrations;
    use crate::test_support::scratch_database;
    use kanban_app::CloneGuardStore;
    use kanban_app::TimelineEnvelope;
    use serde_json::json;

    #[test]
    fn appended_rows_land_in_the_project_timeline() {
        let (_dir, mut database) = scratch_database();
        database
            .migrate(&AllowAllMigrations)
            .expect("the migrations apply");
        let store = SqliteCloneGuardStore::new(&database);

        store
            .append(TimelineEnvelope::project(
                1,
                TimelineEventKind::Transition,
                Some(TimelineEntityRef {
                    kind: TimelineEntityKind::Project,
                    id: "1".to_owned(),
                }),
                json!({
                    "action": "branch_clone_created",
                    "path": "/workspaces/kanban.fleet-t34",
                    "branch": "fleet/kan-t34",
                }),
            ))
            .expect("the invocation row lands");
        store
            .append(TimelineEnvelope::project(
                1,
                TimelineEventKind::Transition,
                Some(TimelineEntityRef {
                    kind: TimelineEntityKind::Workspace,
                    id: "2".to_owned(),
                }),
                json!({
                    "action": "clone_remove_refused",
                    "reason": "lane_assigned",
                    "lane_id": 3,
                }),
            ))
            .expect("the refusal row lands");

        let rows: Vec<(String, String, Option<String>)> = {
            let conn = database.connection();
            let mut statement = conn
                .prepare(
                    "SELECT kind, entity_kind, json_extract(detail, '$.action')
                     FROM timeline_events ORDER BY id",
                )
                .expect("the timeline is readable");
            statement
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
                .expect("the query runs")
                .collect::<Result<Vec<_>, _>>()
                .expect("the rows decode")
        };
        assert_eq!(
            rows,
            vec![
                (
                    "transition".to_owned(),
                    "project".to_owned(),
                    Some("branch_clone_created".to_owned()),
                ),
                (
                    "transition".to_owned(),
                    "workspace".to_owned(),
                    Some("clone_remove_refused".to_owned()),
                ),
            ],
            "invocation and refusal rows are durable timeline rows"
        );
    }
}
