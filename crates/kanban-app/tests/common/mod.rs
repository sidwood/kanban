//! Shared SQLite Core wiring for Dispatch Request tests.

#![allow(dead_code)]

use std::num::NonZeroU32;
use std::sync::{Arc, Mutex};

use kanban_app::catalog::exposed_operations;
use kanban_app::dispatch::Core;
use kanban_app::events::NoopEventSink;
use kanban_app::{CoordinatorWake, CoordinatorWakeRequest, ProfileStore, ProjectStore};
use kanban_domain::{ExecutionProfile, ProfileDefinition, ProfileName, ProjectRegistration};
use kanban_dto::{TimelineEntityKind, TimelineEntityRef, TimelineEventKind};
use kanban_storage::{
    AllowAllMigrations, Database, RetentionPolicy, SqliteCapacityStore, SqliteDependencyStore,
    SqliteDispatchStore, SqliteIdempotencyStore, SqliteLaneStore, SqliteProfileStore,
    SqliteProjectStore, SqliteTicketStore,
};
use serde_json::json;
use tempfile::TempDir;

use kanban_app::TimelineEnvelope;

/// A wake port that records every Coordinator wake.
#[derive(Default)]
pub struct RecordingWake {
    pub calls: Mutex<Vec<CoordinatorWakeRequest>>,
}

impl CoordinatorWake for RecordingWake {
    fn wake(&self, request: CoordinatorWakeRequest) {
        self.calls
            .lock()
            .expect("the wake log is sound")
            .push(request);
    }
}

pub struct DispatchHarness {
    pub _dir: TempDir,
    pub core: Core,
    pub wake: Arc<RecordingWake>,
    pub database_path: std::path::PathBuf,
}

pub fn harness() -> DispatchHarness {
    let dir = TempDir::new().expect("a scratch directory is available");
    let database_path = dir.path().join("kanban.sqlite");
    let mut database = Database::open(&database_path).expect("a scratch database opens");
    database
        .migrate(&AllowAllMigrations)
        .expect("the migrations apply");
    seed_project_profile(&database);

    let projects = Arc::new(SqliteProjectStore::new(&database));
    let tickets = Arc::new(SqliteTicketStore::new(&database));
    let profiles = Arc::new(SqliteProfileStore::new(&database));
    let capacity = Arc::new(SqliteCapacityStore::new(&database));
    let lanes = Arc::new(SqliteLaneStore::new(&database));
    let dependencies = Arc::new(SqliteDependencyStore::new(&database));
    let requests = Arc::new(SqliteDispatchStore::new(&database));
    let wake = Arc::new(RecordingWake::default());
    let idempotency = Arc::new(SqliteIdempotencyStore::new(
        &database,
        RetentionPolicy::keep_most_recent(NonZeroU32::new(100).expect("the bound is not zero")),
    ));
    let mut core = Core::new(exposed_operations(), idempotency, Arc::new(NoopEventSink));
    core.register_dispatch(
        requests,
        tickets,
        profiles,
        projects,
        capacity,
        lanes,
        dependencies,
        wake.clone(),
    )
    .expect("the dispatch operations register");
    DispatchHarness {
        _dir: dir,
        core,
        wake,
        database_path,
    }
}

fn seed_project_profile(database: &Database) {
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
    let profiles = SqliteProfileStore::new(database);
    let profile = ExecutionProfile::define(
        ProfileName::new("standard").expect("the name validates"),
        ProfileDefinition::new("claude-code", "opus", "high", "operator", None)
            .expect("the definition validates"),
    )
    .expect("the profile defines");
    profiles
        .define(
            &profile,
            &TimelineEnvelope::global(
                TimelineEventKind::Transition,
                Some(TimelineEntityRef {
                    kind: TimelineEntityKind::Profile,
                    id: "standard".to_owned(),
                }),
                json!({ "action": "defined" }),
            ),
        )
        .expect("the profile lands");
}

pub fn insert_ticket(database_path: &std::path::Path, number: u64, priority: &str) -> u64 {
    let conn = rusqlite::Connection::open(database_path).expect("the database reopens");
    conn.execute(
        "INSERT INTO tickets
             (project_id, number, kind, priority, state, title, criteria,
              subtype, mode, completion, profile, version)
         VALUES (1, ?1, 'task', ?2, 'draft', 'One slice', '[]',
                 'operational', 'human', '[\"done\"]', 'standard', 1)",
        rusqlite::params![number as i64, priority],
    )
    .expect("the fixture Ticket lands");
    conn.last_insert_rowid()
        .try_into()
        .expect("the Ticket identity fits")
}

pub fn mutation(version: u64, key: impl AsRef<str>) -> serde_json::Value {
    json!({
        "optimistic_version": version,
        "idempotency_key": key.as_ref(),
    })
}

pub fn constrain_harness(database_path: &std::path::Path, cap: u64) {
    let conn = rusqlite::Connection::open(database_path).expect("the database reopens");
    conn.execute(
        "UPDATE capacity_global_defaults SET max_active_per_harness = ?1",
        rusqlite::params![cap as i64],
    )
    .expect("the harness cap applies");
}

pub fn insert_blocker(database_path: &std::path::Path, ticket_id: u64) {
    let conn = rusqlite::Connection::open(database_path).expect("the database reopens");
    conn.execute(
        "INSERT INTO ticket_blockers (ticket_id, description)
         VALUES (?1, 'waiting on an unregistered vendor')",
        rusqlite::params![ticket_id as i64],
    )
    .expect("the blocker lands");
}
