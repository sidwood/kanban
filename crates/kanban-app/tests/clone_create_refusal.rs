//! App gate for the existing-target refusal (KAN-T118, DR-LW-10): a
//! clone sitting unregistered at the target — the shape `git bc-add`
//! treats as success while it syncs extras into it — must be refused
//! before the skill runs, leaving no creation row and no live event,
//! through the real Core dispatch, the shipped filesystem probe, and
//! the SQLite clone-guard timeline.

use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};

use kanban_app::FleetCloneTool;
use kanban_app::catalog::exposed_operations;
use kanban_app::dispatch::Core;
use kanban_app::events::EventSink;
use kanban_app::project::ProjectStore;
use kanban_app::timeline::TimelineEnvelope;
use kanban_domain::ProjectRegistration;
use kanban_dto::{ApiError, ErrorCode, TimelineEntityKind, TimelineEntityRef, TimelineEventKind};
use kanban_service::LocalCloneTargetProbe;
use kanban_storage::{
    AllowAllMigrations, Database, RetentionPolicy, SqliteCloneGuardStore, SqliteIdempotencyStore,
    SqliteProjectStore, SqliteWorkspaceStore,
};
use serde_json::{Value, json};
use tempfile::TempDir;

/// The fleet skill stand-in: it records every invocation and accepts
/// everything, so the guard's ordering — refuse before invoking — is
/// the only thing under test here.
#[derive(Default)]
struct CountingTool {
    calls: Mutex<Vec<String>>,
}

impl FleetCloneTool for CountingTool {
    fn add_clone(&self, source: &str, path: &str, branch: &str) -> Result<(), ApiError> {
        self.calls
            .lock()
            .expect("the tool lock is sound")
            .push(format!("add {source} {path} {branch}"));
        Ok(())
    }

    fn remove_clone(&self, path: &str) -> Result<(), ApiError> {
        self.calls
            .lock()
            .expect("the tool lock is sound")
            .push(format!("remove {path}"));
        Ok(())
    }
}

/// Records every live event the core publishes.
#[derive(Default)]
struct RecordingSink {
    events: Mutex<Vec<(String, Value)>>,
}

impl EventSink for RecordingSink {
    fn emit(&self, event_type: &str, payload: Value) {
        self.events
            .lock()
            .expect("the sink lock is sound")
            .push((event_type.to_owned(), payload));
    }
}

struct Wired {
    core: Core,
    projects: Arc<SqliteProjectStore>,
    tool: Arc<CountingTool>,
    sink: Arc<RecordingSink>,
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
    let clone_guard = Arc::new(SqliteCloneGuardStore::new(&database));
    let idempotency = Arc::new(SqliteIdempotencyStore::new(
        &database,
        RetentionPolicy::keep_most_recent(NonZeroU32::new(100).expect("the bound is not zero")),
    ));
    let tool = Arc::new(CountingTool::default());
    let sink = Arc::new(RecordingSink::default());
    let mut core = Core::new(exposed_operations(), idempotency, sink.clone());
    core.register_clones(
        tool.clone(),
        projects.clone(),
        workspaces,
        clone_guard,
        Arc::new(LocalCloneTargetProbe),
    )
    .expect("the clone operations register");
    Wired {
        core,
        projects,
        tool,
        sink,
        database_path,
    }
}

/// Seed the fixture Project directly through the store.
fn create_project(projects: &SqliteProjectStore, repository: &str, seed_workspace: &str) {
    let registration = ProjectRegistration::new(
        "CORE",
        "Control plane",
        repository,
        seed_workspace,
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

fn git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .args(args)
        .current_dir(dir)
        .status()
        .expect("git runs");
    assert!(status.success(), "git {:?} in {}", args, dir.display());
}

fn init_repo(dir: &Path) -> String {
    git(dir, &["init"]);
    git(dir, &["config", "user.email", "test@example.com"]);
    git(dir, &["config", "user.name", "Test"]);
    std::fs::write(dir.join("README.md"), "seed\n").expect("the seed file is written");
    git(dir, &["add", "."]);
    git(dir, &["commit", "-m", "initial"]);
    dir.to_str().expect("the path is UTF-8").to_owned()
}

fn create(path: &str, branch: &str, key: &str) -> Value {
    json!({
        "mutation": { "optimistic_version": 0, "idempotency_key": key },
        "project_id": 1,
        "path": path,
        "branch": branch,
    })
}

/// The timeline actions one Project's rows carry, in landing order.
fn recorded_actions(database_path: &Path) -> Vec<String> {
    rusqlite::Connection::open(database_path)
        .expect("the database reopens")
        .prepare("SELECT json_extract(detail, '$.action') FROM timeline_events ORDER BY id")
        .expect("the timeline is readable")
        .query_map([], |row| row.get(0))
        .expect("the query runs")
        .collect::<Result<Vec<_>, _>>()
        .expect("the rows decode")
}

#[test]
fn clone_create_refuses_a_real_unregistered_clone_at_the_target() {
    let scratch = TempDir::new().expect("a scratch directory is available");
    let Wired {
        core,
        projects,
        tool,
        sink,
        database_path,
    } = wired(scratch.path());

    let git_dir = TempDir::new().expect("a scratch directory is available");
    let repository = init_repo(git_dir.path());
    // The clone made outside Kanban: sanctioned branch-clone shape,
    // registered to no Workspace, exactly the occupant Kanban must not
    // claim it created.
    let target = git_dir.path().join("kanban.fleet-t34");
    git(
        Path::new(&repository),
        &["clone", "--local", &repository, target.to_str().unwrap()],
    );
    git(
        &target,
        &[
            "config",
            "bc.source",
            Path::new(&repository)
                .canonicalize()
                .expect("the repository resolves")
                .to_str()
                .expect("the path is UTF-8"),
        ],
    );
    create_project(&projects, &repository, &repository);

    let error = core
        .command(
            "clone.create",
            &create(target.to_str().unwrap(), "fleet/kan-t118", "create-key"),
        )
        .expect_err("the existing clone is refused before the skill runs");

    assert_eq!(error.code, ErrorCode::InvalidRequest);
    assert!(
        error.message.contains("registered to no Workspace"),
        "the refusal names the rule: {}",
        error.message
    );
    assert!(
        error
            .message
            .contains(target.to_str().expect("the path is UTF-8")),
        "the refusal names the target: {}",
        error.message
    );
    assert!(
        tool.calls
            .lock()
            .expect("the tool lock is sound")
            .is_empty(),
        "the fleet skill is never invoked for an occupied target"
    );
    let events = sink.events.lock().expect("the sink lock is sound").clone();
    assert!(
        events.iter().all(|(name, _)| name != "clone.created"),
        "a refused target announces no clone.created live event: {events:?}"
    );
    assert_eq!(
        recorded_actions(&database_path),
        vec!["registered".to_owned(), "clone_create_refused".to_owned()],
        "the refusal row lands; no branch_clone_created row ever does"
    );
}

#[test]
fn a_free_target_still_reaches_the_fleet_skill_through_the_real_probe() {
    let scratch = TempDir::new().expect("a scratch directory is available");
    let Wired {
        core,
        projects,
        tool,
        sink,
        database_path,
    } = wired(scratch.path());

    let git_dir = TempDir::new().expect("a scratch directory is available");
    let repository = init_repo(git_dir.path());
    let target = git_dir.path().join("kanban.fleet-t34");
    create_project(&projects, &repository, &repository);

    let response = core
        .command(
            "clone.create",
            &create(target.to_str().unwrap(), "fleet/kan-t118", "create-key"),
        )
        .expect("a free target reaches the skill");

    assert_eq!(
        response["path"],
        json!(target.to_str().expect("the path is UTF-8"))
    );
    assert_eq!(
        tool.calls.lock().expect("the tool lock is sound").len(),
        1,
        "the probe refuses occupancy, never the command itself"
    );
    let events = sink.events.lock().expect("the sink lock is sound").clone();
    assert_eq!(
        events
            .iter()
            .filter(|(name, _)| name == "clone.created")
            .count(),
        1,
        "the one real creation still announces itself"
    );
    assert_eq!(
        recorded_actions(&database_path),
        vec!["registered".to_owned(), "branch_clone_created".to_owned(),],
        "the invocation row lands for a target that was genuinely free"
    );
}
