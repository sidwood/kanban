//! Shared SQLite Core wiring for Dispatch Request tests.

#![allow(dead_code)]

pub mod landing_review;
pub mod review;

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
    SqliteDispatchStore, SqliteGraphProposalStore, SqliteIdempotencyStore, SqliteLaneStore,
    SqliteProfileStore, SqliteProjectStore, SqliteRunStore, SqliteTicketStore,
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
    pub database: Database,
}

pub fn harness() -> DispatchHarness {
    let dir = TempDir::new().expect("a scratch directory is available");
    let database_path = dir.path().join("kanban.sqlite");
    let mut database = Database::open(&database_path).expect("a scratch database opens");
    database
        .migrate(&AllowAllMigrations)
        .expect("the migrations apply");
    seed_project_profile(&database);
    let (core, wake) = core_over(&database);
    DispatchHarness {
        _dir: dir,
        core,
        wake,
        database_path,
        database,
    }
}

/// The dispatch, review, run, and submission operations wired over
/// one already-migrated database, so a test can reopen the same
/// file and serve it through a fresh Core.
pub fn core_over(database: &Database) -> (Core, Arc<RecordingWake>) {
    let projects = Arc::new(SqliteProjectStore::new(database));
    let tickets = Arc::new(SqliteTicketStore::new(database));
    let profiles = Arc::new(SqliteProfileStore::new(database));
    let capacity = Arc::new(SqliteCapacityStore::new(database));
    let lanes = Arc::new(SqliteLaneStore::new(database));
    let dependencies = Arc::new(SqliteDependencyStore::new(database));
    let requests = Arc::new(SqliteDispatchStore::new(database));
    let runs = Arc::new(SqliteRunStore::new(database));
    let proposals = Arc::new(SqliteGraphProposalStore::new(database));
    let wake = Arc::new(RecordingWake::default());
    let idempotency = Arc::new(SqliteIdempotencyStore::new(
        database,
        RetentionPolicy::keep_most_recent(NonZeroU32::new(100).expect("the bound is not zero")),
    ));
    let mut core = Core::new(exposed_operations(), idempotency, Arc::new(NoopEventSink));
    core.register_dispatch(
        requests.clone(),
        tickets.clone(),
        profiles.clone(),
        projects.clone(),
        capacity,
        lanes,
        dependencies.clone(),
        proposals.clone(),
        wake.clone(),
    )
    .expect("the dispatch operations register");
    core.register_deferrals(
        Arc::new(kanban_storage::SqliteDeferralStore::new(database)),
        Arc::new(kanban_storage::SqliteProjectStore::new(database)),
    )
    .unwrap();
    core.register_deferral_promotions(
        Arc::new(kanban_storage::SqliteFindingStore::new(database)),
        Arc::new(kanban_storage::SqliteDeferralStore::new(database)),
        Arc::new(kanban_storage::SqliteProjectStore::new(database)),
        Arc::new(kanban_storage::SqliteTicketStore::new(database)),
        Arc::new(kanban_storage::SqliteSpecStore::new(database)),
    )
    .unwrap();
    core.register_findings(
        Arc::new(kanban_storage::SqliteFindingStore::new(database)),
        Arc::new(kanban_storage::SqliteProjectStore::new(database)),
    )
    .unwrap();
    core.register_reviews(
        Arc::new(kanban_storage::SqliteReviewExecutionStore::new(database)),
        Arc::new(kanban_storage::SqliteReviewConfigStore::new(database)),
        Arc::new(kanban_storage::SqliteTicketStore::new(database)),
        Arc::new(kanban_storage::SqliteProfileStore::new(database)),
        Arc::new(kanban_storage::SqliteProjectStore::new(database)),
        Arc::new(kanban_storage::SqliteSubmissionStore::new(database)),
        Arc::new(kanban_storage::SqliteRunStore::new(database)),
        wake.clone(),
    )
    .unwrap();
    core.register_runs(
        runs,
        requests,
        tickets,
        profiles,
        projects,
        dependencies,
        proposals,
    )
    .expect("the run operations register");
    core.register_submissions(
        Arc::new(kanban_storage::SqliteSubmissionStore::new(database)),
        Arc::new(kanban_storage::SqliteCapabilityStore::new(database)),
        Arc::new(kanban_storage::SqliteProjectStore::new(database)),
        wake.clone(),
    )
    .expect("the submission operations register");
    (core, wake)
}

pub fn seed_project_profile(database: &Database) {
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

/// Seat a fixture Task Ticket as quick capture leaves it: draft, so
/// the lifecycle fixtures can move it themselves. Every fixture Task
/// is agent-mode: human-mode work is Sid's own and acquires no
/// implementer authority (KAN-T138).
pub fn insert_ticket(database_path: &std::path::Path, number: u64, priority: &str) -> u64 {
    insert_ticket_with_profile(database_path, number, priority, "standard")
}

/// Seat a draft fixture Ticket under a named profile; the seeded
/// catalogue carries `standard` and tests define any other entry they
/// name.
pub fn insert_ticket_with_profile(
    database_path: &std::path::Path,
    number: u64,
    priority: &str,
    profile: &str,
) -> u64 {
    insert_ticket_row(database_path, number, priority, profile, "draft")
}

/// Seat a fixture Task Ticket already ready: the executable state
/// ordinary admission requires before a claim or an acknowledgement
/// admits a run (KAN-T138).
pub fn insert_ready_ticket(database_path: &std::path::Path, number: u64, priority: &str) -> u64 {
    insert_ready_ticket_with_profile(database_path, number, priority, "standard")
}

/// Seat a ready fixture Ticket under a named profile.
pub fn insert_ready_ticket_with_profile(
    database_path: &std::path::Path,
    number: u64,
    priority: &str,
    profile: &str,
) -> u64 {
    insert_ticket_row(database_path, number, priority, profile, "ready")
}

fn insert_ticket_row(
    database_path: &std::path::Path,
    number: u64,
    priority: &str,
    profile: &str,
    state: &str,
) -> u64 {
    let conn = rusqlite::Connection::open(database_path).expect("the database reopens");
    conn.execute(
        "INSERT INTO tickets
             (project_id, number, kind, priority, state, title, criteria,
              subtype, mode, completion, profile, version)
         VALUES (1, ?1, 'task', ?2, ?4, 'One slice', '[]',
                 'operational', 'agent', '[\"done\"]', ?3, 1)",
        rusqlite::params![number as i64, priority, profile, state],
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
    constrain_global(database_path, "max_active_per_harness", cap);
}

/// Set one global capacity default. `dimension` names the column.
pub fn constrain_global(database_path: &std::path::Path, dimension: &str, cap: u64) {
    let conn = rusqlite::Connection::open(database_path).expect("the database reopens");
    conn.execute(
        &format!("UPDATE capacity_global_defaults SET {dimension} = ?1"),
        rusqlite::params![cap as i64],
    )
    .expect("the global cap applies");
}

/// Impose one Project cap on the seeded Project. `dimension` names
/// the column.
pub fn cap_project(database_path: &std::path::Path, dimension: &str, cap: u64) {
    let conn = rusqlite::Connection::open(database_path).expect("the database reopens");
    conn.execute(
        &format!(
            "INSERT INTO capacity_project_caps (project_id, {dimension}, version)
             VALUES (1, ?1, 1)"
        ),
        rusqlite::params![cap as i64],
    )
    .expect("the Project cap applies");
}

/// Seat `ticket_id` in a fresh Lane holding no Workspace: the Ticket
/// is assigned, which is the fact the capacity claim reads.
pub fn assign_lane(database_path: &std::path::Path, ticket_id: u64) {
    let conn = rusqlite::Connection::open(database_path).expect("the database reopens");
    conn.execute(
        "INSERT INTO lanes (project_id, ticket_id, version) VALUES (1, ?1, 1)",
        rusqlite::params![ticket_id as i64],
    )
    .expect("the fixture Lane lands");
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
