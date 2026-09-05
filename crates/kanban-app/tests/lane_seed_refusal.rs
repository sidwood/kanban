//! App gate for the durable Seed refusal (KAN-T32, DR-LW-07): a
//! refused Seed assignment must leave its timeline row committed
//! through the real `Core::command` dispatch and the SQLite Lane
//! store, even though the failed command's own writes roll back with
//! its discarded mutation span.

use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use kanban_app::catalog::exposed_operations;
use kanban_app::dispatch::Core;
use kanban_app::events::NoopEventSink;
use kanban_app::project::ProjectStore;
use kanban_app::timeline::TimelineEnvelope;
use kanban_domain::ProjectRegistration;
use kanban_dto::{ErrorCode, TimelineEntityKind, TimelineEntityRef, TimelineEventKind};
use kanban_service::git_observer::LocalWorkspaceGitObserver;
use kanban_storage::{
    AllowAllMigrations, Database, RetentionPolicy, SqliteIdempotencyStore, SqliteLaneStore,
    SqliteProjectStore, SqliteTicketStore, SqliteWorkspaceStore,
};
use rusqlite::params;
use serde_json::{Value, json};
use tempfile::TempDir;

struct Wired {
    core: Core,
    projects: Arc<SqliteProjectStore>,
    database_path: PathBuf,
}

fn wired(scratch: &Path) -> Wired {
    let database_path = scratch.join("kanban.sqlite");
    let mut database = Database::open(&database_path).expect("a scratch database opens");
    database
        .migrate(&AllowAllMigrations)
        .expect("the migrations apply");
    let projects = Arc::new(SqliteProjectStore::new(&database));
    let workspaces = Arc::new(SqliteWorkspaceStore::new(&database));
    let lanes = Arc::new(SqliteLaneStore::new(&database));
    let tickets = Arc::new(SqliteTicketStore::new(&database));
    let idempotency = Arc::new(SqliteIdempotencyStore::new(
        &database,
        RetentionPolicy::keep_most_recent(NonZeroU32::new(100).expect("the bound is not zero")),
    ));
    let mut core = Core::new(exposed_operations(), idempotency, Arc::new(NoopEventSink));
    core.register_workspaces(
        workspaces.clone(),
        projects.clone(),
        Arc::new(LocalWorkspaceGitObserver),
    )
    .expect("the workspace operations register");
    core.register_lanes(lanes, projects.clone(), workspaces, tickets)
        .expect("the lane operations register");
    Wired {
        core,
        projects,
        database_path,
    }
}

/// Seed the fixture Project directly through the store: its Seed
/// Workspace path is `/workspaces/kanban.seed`.
fn create_project(projects: &SqliteProjectStore) {
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
                json!({ "action": "registered", "code": "CORE", "id": id.value() }),
            )
        })
        .expect("the project registers");
}

fn mutation(version: u64, key: &str) -> Value {
    json!({ "optimistic_version": version, "idempotency_key": key })
}

#[test]
fn a_refused_seed_assignment_keeps_its_record_and_discards_its_mutation() {
    let scratch = TempDir::new().expect("a scratch directory is available");
    let Wired {
        core,
        projects,
        database_path,
    } = wired(scratch.path());
    create_project(&projects);

    let lane = core
        .command(
            "lane.create",
            &json!({
                "mutation": mutation(0, "lane-create-key"),
                "project_id": 1,
            }),
        )
        .expect("the lane creates");
    let lane_id = lane["id"].as_u64().expect("the lane identity is a number");
    core.command(
        "workspace.register",
        &json!({
            "mutation": mutation(0, "seed-register-key"),
            "project_id": 1,
            "path": "/workspaces/kanban.seed",
        }),
    )
    .expect("the Seed Workspace registers");

    let error = core
        .command(
            "lane.workspace.assign",
            &json!({
                "mutation": mutation(1, "seed-refusal-key"),
                "lane_id": lane_id,
                "workspace_id": 1,
            }),
        )
        .expect_err("the Seed assignment is refused");
    assert_eq!(error.code, ErrorCode::InvalidRequest);
    assert!(
        error.message.contains("never be an execution Lane"),
        "the refusal names the rule: {}",
        error.message
    );

    // The refusal row must be durable: a connection opened after the
    // failed command reads it back from the file, proving the record
    // committed instead of rolling back with the rejected mutation.
    let conn = rusqlite::Connection::open(&database_path).expect("the database reopens");
    let refusal_rows: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM timeline_events
              WHERE entity_kind = 'lane'
                AND json_extract(detail, '$.action') = 'seed_assignment_refused'",
            [],
            |row| row.get(0),
        )
        .expect("the timeline is readable");
    assert_eq!(
        refusal_rows, 1,
        "the recorded refusal must survive the failed command's rollback (DR-LW-07)"
    );
    let refusal: (String, String) = conn
        .query_row(
            "SELECT json_extract(detail, '$.path'), json_extract(detail, '$.reason')
             FROM timeline_events
              WHERE entity_kind = 'lane'
                AND json_extract(detail, '$.action') = 'seed_assignment_refused'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("the refusal row is readable");
    assert_eq!(
        refusal,
        ("/workspaces/kanban.seed".to_owned(), "seed".to_owned(),),
        "the refusal records the attempted path and the reason"
    );

    // Nothing the rejected command wrote committed: no Lane claim, no
    // version move, no Workspace mirror, no spent idempotency key.
    let lane_row: (Option<i64>, i64) = conn
        .query_row(
            "SELECT workspace_id, version FROM lanes WHERE id = ?1",
            params![lane_id as i64],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("the lane row is readable");
    assert_eq!(
        lane_row,
        (None, 1),
        "the refused assignment changed no Lane row"
    );
    let workspace_lane: Option<i64> = conn
        .query_row("SELECT lane_id FROM workspaces WHERE id = 1", [], |row| {
            row.get(0)
        })
        .expect("the workspace row is readable");
    assert_eq!(workspace_lane, None, "the Seed stays unclaimed");
    let spent: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM idempotency_outcomes
              WHERE idempotency_key = 'seed-refusal-key'",
            [],
            |row| row.get(0),
        )
        .expect("the idempotency index is readable");
    assert_eq!(spent, 0, "the refused command spends no idempotency key");
    drop(conn);

    // The unspent key still applies: the refusal left the core, the
    // Lane, and the key exactly as they were.
    core.command(
        "workspace.register",
        &json!({
            "mutation": mutation(0, "feature-register-key"),
            "project_id": 1,
            "path": "/workspaces/kanban.feature",
        }),
    )
    .expect("the clone Workspace registers");
    let assigned = core
        .command(
            "lane.workspace.assign",
            &json!({
                "mutation": mutation(1, "seed-refusal-key"),
                "lane_id": lane_id,
                "workspace_id": 2,
            }),
        )
        .expect("the refused command's unspent key still applies");
    assert_eq!(assigned["workspace_id"], json!(2));
}
