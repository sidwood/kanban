//! App gate for the clone command audit evidence (KAN-T128): through
//! the real Core dispatch and the SQLite clone-guard timeline, an
//! invoked skill that fails records the invocation beside the failure,
//! a storage failure after a successful skill cannot erase the
//! invocation, and an archived-Project refusal lands durably while
//! invoking nothing.

use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use kanban_app::catalog::exposed_operations;
use kanban_app::clone::PROJECT_ARCHIVED;
use kanban_app::dispatch::Core;
use kanban_app::events::EventSink;
use kanban_app::project::ProjectStore;
use kanban_app::timeline::TimelineEnvelope;
use kanban_app::{CloneGuardStore, FleetCloneTool};
use kanban_domain::{ProjectId, ProjectRegistration};
use kanban_dto::{ApiError, ErrorCode, TimelineEntityKind, TimelineEntityRef, TimelineEventKind};
use kanban_service::LocalCloneTargetProbe;
use kanban_storage::{
    AllowAllMigrations, Database, RetentionPolicy, SqliteCloneGuardStore, SqliteIdempotencyStore,
    SqliteProjectStore, SqliteWorkspaceStore,
};
use serde_json::{Value, json};
use tempfile::TempDir;

/// The fleet skill stand-in: it records every invocation and answers
/// from a script of at most one planted failure, so the guard's audit
/// rows are the only thing under test here.
#[derive(Default)]
struct CountingTool {
    calls: Mutex<Vec<String>>,
    fail_add_with: Mutex<Option<ApiError>>,
}

impl FleetCloneTool for CountingTool {
    fn add_clone(&self, source: &str, path: &str, branch: &str) -> Result<(), ApiError> {
        self.calls
            .lock()
            .expect("the tool lock is sound")
            .push(format!("add {source} {path} {branch}"));
        if std::env::var_os("KANBAN_TEST_CRASH_AFTER_CLONE").is_some() {
            // Real Git in the fixture; process death bypasses all Rust drops.
            git(
                Path::new(source),
                &["clone", "--no-hardlinks", source, path],
            );
            git(Path::new(path), &["checkout", "-b", branch]);
            git(Path::new(path), &["config", "bc.source", source]);
            std::process::exit(37);
        }
        match self
            .fail_add_with
            .lock()
            .expect("the tool lock is sound")
            .take()
        {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    fn remove_clone(&self, path: &str) -> Result<(), ApiError> {
        self.calls
            .lock()
            .expect("the tool lock is sound")
            .push(format!("remove {path}"));
        Ok(())
    }
}

/// The clone-guard timeline over the real store, whose first append
/// fails the way a storage layer would when the test arms it, so one
/// chosen row is lost and every later row still lands.
struct SteeredTimeline {
    inner: SqliteCloneGuardStore,
    fail_first: AtomicBool,
}

impl CloneGuardStore for SteeredTimeline {
    fn prepare_creation(&self, intent: &kanban_dto::CloneRecoveryRecord) -> Result<(), ApiError> {
        self.inner.prepare_creation(intent)
    }
    fn complete_creation(
        &self,
        key: &str,
        workspace_id: kanban_domain::WorkspaceId,
    ) -> Result<(), ApiError> {
        self.inner.complete_creation(key, workspace_id)
    }
    fn pending_creations(
        &self,
        project_id: ProjectId,
    ) -> Result<Vec<kanban_dto::CloneRecoveryRecord>, ApiError> {
        self.inner.pending_creations(project_id)
    }
    fn append(&self, envelope: TimelineEnvelope) -> Result<(), ApiError> {
        if self.fail_first.swap(false, Ordering::SeqCst) {
            return Err(ApiError::internal("the timeline row could not be written"));
        }
        self.inner.append(envelope)
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

/// Wire the real Core over a migrated scratch database, arming the
/// clone-guard timeline's first-append failure when `fail_first` asks
/// for it.
fn wired(scratch: &Path, fail_first: bool) -> Wired {
    let database_path = scratch.join("kanban.sqlite");
    let mut database = Database::open(&database_path).expect("a scratch database opens");
    database
        .migrate(&AllowAllMigrations)
        .expect("the migrations apply");
    let projects = Arc::new(SqliteProjectStore::new(&database));
    let workspaces = Arc::new(SqliteWorkspaceStore::new(&database));
    let timeline = Arc::new(SteeredTimeline {
        inner: SqliteCloneGuardStore::new(&database),
        fail_first: AtomicBool::new(fail_first),
    });
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
        timeline,
        Arc::new(LocalCloneTargetProbe),
        Arc::new(kanban_service::git_observer::LocalWorkspaceGitObserver),
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
fn create_project(projects: &SqliteProjectStore) {
    create_project_at(projects, "/repositories/kanban", "/workspaces/kanban.seed");
}

fn create_project_at(projects: &SqliteProjectStore, repository: &str, seed: &str) {
    let registration = ProjectRegistration::new(
        "CORE",
        "Control plane",
        repository,
        seed,
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

/// Archive the seeded Project through the store, so the refusal under
/// test is the archive rule alone.
fn archive_project(projects: &SqliteProjectStore) {
    let id = ProjectId::new(1);
    let mut project = projects
        .find(id)
        .expect("the project loads")
        .expect("the seeded project exists");
    project.archive().expect("the project archives");
    projects
        .save(
            &project,
            TimelineEnvelope::project(
                id.value(),
                TimelineEventKind::Transition,
                Some(TimelineEntityRef {
                    kind: TimelineEntityKind::Project,
                    id: id.value().to_string(),
                }),
                json!({ "action": "archived" }),
            ),
        )
        .expect("the archived project saves");
}

fn create(path: &str, branch: &str, key: &str) -> Value {
    json!({
        "mutation": { "optimistic_version": 0, "idempotency_key": key },
        "project_id": 1,
        "path": path,
        "branch": branch,
    })
}

/// Every timeline row's action and detail, in landing order.
fn recorded_rows(database_path: &Path) -> Vec<(String, Value)> {
    rusqlite::Connection::open(database_path)
        .expect("the database reopens")
        .prepare(
            "SELECT json_extract(detail, '$.action'), detail
             FROM timeline_events ORDER BY id",
        )
        .expect("the timeline is readable")
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .expect("the query runs")
        .collect::<Result<Vec<_>, _>>()
        .expect("the rows decode")
        .into_iter()
        .map(|(action, detail)| {
            (
                action,
                serde_json::from_str(&detail).expect("the detail is JSON"),
            )
        })
        .collect()
}

/// KAN-T128-AC1: a skill failure after invocation lands the truth as
/// durable rows in order — the invocation, then the failure it
/// explains — never a lone refusal that hides that the skill ran.
#[test]
fn a_failed_invocation_lands_the_invocation_and_failure_in_order() {
    let scratch = TempDir::new().expect("a scratch directory is available");
    let Wired {
        core,
        projects,
        tool,
        sink,
        database_path,
    } = wired(scratch.path(), false);
    create_project(&projects);
    tool.fail_add_with
        .lock()
        .expect("the tool lock is sound")
        .replace(ApiError::internal(
            "the fleet clone skill `git bc-add` failed: fatal: could not read from remote repository",
        ));

    let error = core
        .command(
            "clone.create",
            &create("/workspaces/kanban.fleet-t34", "fleet/kan-t34", "audit-key"),
        )
        .expect_err("the failed invocation refuses the command");

    assert_eq!(error.code, ErrorCode::Internal);
    assert_eq!(
        tool.calls.lock().expect("the tool lock is sound").len(),
        1,
        "the skill was invoked"
    );
    let rows = recorded_rows(&database_path);
    assert_eq!(
        rows.iter()
            .map(|(action, _)| action.as_str())
            .collect::<Vec<_>>(),
        vec!["registered", "clone_create_invoked", "clone_create_failed"],
        "the invocation lands before the failure it explains, durably"
    );
    let (_, invoked) = &rows[1];
    assert_eq!(invoked["path"], json!("/workspaces/kanban.fleet-t34"));
    assert_eq!(invoked["branch"], json!("fleet/kan-t34"));
    assert_eq!(invoked["source"], json!("/repositories/kanban"));
    let (_, failed) = &rows[2];
    assert_eq!(failed["reason"], json!("fleet_tool_failed"));
    assert!(
        failed["error"]
            .as_str()
            .expect("the error text is recorded")
            .contains("could not read from remote repository"),
        "the durable row carries the sanitised failure: {failed}"
    );
    let events = sink.events.lock().expect("the sink lock is sound").clone();
    assert!(
        events.iter().all(|(name, _)| name != "clone.created"),
        "a failed invocation announces no clone.created live event: {events:?}"
    );
}

/// KAN-T128-AC1: a storage failure after a successful skill cannot
/// erase the invocation. The creation row is the append that fails, so
/// the command fails — and the invocation row, written in its own
/// write once the failed mutation has rolled back, remains in the
/// durable timeline as the evidence that the skill ran.
#[test]
fn a_storage_failure_after_a_successful_skill_keeps_the_invocation() {
    let scratch = TempDir::new().expect("a scratch directory is available");
    let Wired {
        core,
        projects,
        tool,
        sink,
        database_path,
    } = wired(scratch.path(), true);
    create_project(&projects);

    let error = core
        .command(
            "clone.create",
            &create("/workspaces/kanban.fleet-t34", "fleet/kan-t34", "audit-key"),
        )
        .expect_err("the unrecordable creation fails the command");

    assert_eq!(error.code, ErrorCode::Internal);
    assert_eq!(
        tool.calls.lock().expect("the tool lock is sound").len(),
        1,
        "the skill ran and succeeded before storage failed"
    );
    let rows = recorded_rows(&database_path);
    assert_eq!(
        rows.iter()
            .map(|(action, _)| action.as_str())
            .collect::<Vec<_>>(),
        vec![
            "registered",
            "clone_create_invoked",
            "clone_adoption_failed"
        ],
        "the invocation evidence remains, and no creation row ever lands"
    );
    let (_, invoked) = &rows[1];
    assert_eq!(invoked["path"], json!("/workspaces/kanban.fleet-t34"));
    assert_eq!(invoked["branch"], json!("fleet/kan-t34"));
    assert_eq!(invoked["source"], json!("/repositories/kanban"));
    let events = sink.events.lock().expect("the sink lock is sound").clone();
    assert!(
        events.iter().all(|(name, _)| name != "clone.created"),
        "an unrecordable creation announces no clone.created live event: {events:?}"
    );
}

/// KAN-T128-AC2: an archived-Project refusal lands a durable refusal
/// row beside the caller's error, while the skill never runs.
#[test]
fn an_archived_project_refusal_lands_durably_and_invokes_nothing() {
    let scratch = TempDir::new().expect("a scratch directory is available");
    let Wired {
        core,
        projects,
        tool,
        sink,
        database_path,
    } = wired(scratch.path(), false);
    create_project(&projects);
    archive_project(&projects);

    let error = core
        .command(
            "clone.create",
            &create("/workspaces/kanban.fleet-t34", "fleet/kan-t34", "audit-key"),
        )
        .expect_err("the archived Project is refused");

    assert_eq!(error.code, ErrorCode::InvalidRequest);
    assert!(
        error.message.contains("archived"),
        "the refusal itself stays observable to its caller: {}",
        error.message
    );
    assert!(
        tool.calls
            .lock()
            .expect("the tool lock is sound")
            .is_empty(),
        "an archived Project invokes nothing"
    );
    let rows = recorded_rows(&database_path);
    assert_eq!(
        rows.iter()
            .map(|(action, _)| action.as_str())
            .collect::<Vec<_>>(),
        vec!["registered", "archived", "clone_create_refused"],
        "the archived refusal lands as a durable row"
    );
    let (_, refused) = &rows[2];
    assert_eq!(refused["reason"], json!(PROJECT_ARCHIVED));
    assert_eq!(refused["path"], json!("/workspaces/kanban.fleet-t34"));
    assert_eq!(refused["branch"], json!("fleet/kan-t34"));
    let events = sink.events.lock().expect("the sink lock is sound").clone();
    assert!(
        events.iter().all(|(name, _)| name != "clone.created"),
        "a refused request announces no clone.created live event: {events:?}"
    );
}

#[test]
fn clone_adoption_failure_keeps_repair_evidence_without_workspace_or_success() {
    let scratch = TempDir::new().unwrap();
    let wired = wired(scratch.path(), false);
    create_project(&wired.projects);
    let conn = rusqlite::Connection::open(&wired.database_path).unwrap();
    conn.execute_batch("CREATE TRIGGER refuse_adoption BEFORE INSERT ON workspaces BEGIN SELECT RAISE(ABORT, 'fixture adoption failure'); END;").unwrap();
    assert!(
        wired
            .core
            .command(
                "clone.create",
                &create(
                    "/workspaces/kanban.test-adoption",
                    "test-adoption",
                    "adopt-key"
                )
            )
            .is_err()
    );
    assert_eq!(wired.tool.calls.lock().unwrap().len(), 1);
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM workspaces", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert!(wired.sink.events.lock().unwrap().is_empty());
    let rows = recorded_rows(&wired.database_path);
    let detail = &rows
        .iter()
        .find(|(action, _)| action == "clone_adoption_failed")
        .expect("a created but unregistered clone has explicit repair evidence")
        .1;
    assert_eq!(detail["path"], "/workspaces/kanban.test-adoption");
    assert_eq!(detail["branch"], "test-adoption");
    assert_eq!(detail["source"], "/repositories/kanban");
}

fn git(root: &Path, args: &[&str]) {
    let output = std::process::Command::new("git")
        .current_dir(root)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn clone_crash_worker() {
    let Some(root) = std::env::var_os("KANBAN_TEST_CRASH_ROOT") else {
        return;
    };
    let root = PathBuf::from(root);
    let repository = root.join("seed");
    std::fs::create_dir(&repository).unwrap();
    git(&repository, &["init", "-b", "main"]);
    git(
        &repository,
        &[
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "commit",
            "--allow-empty",
            "-m",
            "Create the fixture",
        ],
    );
    let repository = repository.canonicalize().unwrap();
    let wired = wired(&root, false);
    create_project_at(
        &wired.projects,
        repository.to_str().unwrap(),
        repository.to_str().unwrap(),
    );
    wired
        .core
        .command(
            "clone.create",
            &create(
                root.join("external").to_str().unwrap(),
                "crash-test",
                "crash-key",
            ),
        )
        .unwrap();
    panic!("the fixture should terminate inside the external clone tool");
}

#[test]
fn clone_creation_intent_survives_process_death_before_adoption() {
    let scratch = TempDir::new().unwrap();
    let child = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "clone_crash_worker", "--nocapture"])
        .env("KANBAN_TEST_CRASH_ROOT", scratch.path())
        .env("KANBAN_TEST_CRASH_AFTER_CLONE", "1")
        .status()
        .unwrap();
    assert_eq!(child.code(), Some(37));
    assert!(scratch.path().join("external").is_dir());
    let restarted = wired(scratch.path(), false);
    let recoveries = restarted
        .core
        .query("clone.recoveries", &json!({"project_id":1}))
        .expect("recovery after process death exposes the durable unadopted clone");
    assert_eq!(recoveries["attempts"].as_array().unwrap().len(), 1);
    assert_eq!(
        recoveries["attempts"][0]["path"],
        scratch.path().join("external").to_str().unwrap()
    );
    assert_eq!(recoveries["attempts"][0]["branch"], "crash-test");
    assert_eq!(recoveries["attempts"][0]["idempotency_key"], "crash-key");
    let conn = rusqlite::Connection::open(&restarted.database_path).unwrap();
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM workspaces", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM idempotency_outcomes", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );

    let request = json!({"mutation":{"optimistic_version":0,"idempotency_key":"adopt-after-crash"},
        "project_id":1,"intent_key":"crash-key"});
    let adopted = restarted
        .core
        .command("clone.adopt", &request)
        .expect("an operator reconciles the surviving clone without invoking creation again");
    assert!(adopted["id"].as_u64().unwrap() > 0);
    assert_eq!(adopted["observation"]["branch"], "crash-test");
    assert_eq!(adopted["health"], "available");
    assert_eq!(
        restarted.core.command("clone.adopt", &request).unwrap(),
        adopted
    );
    assert!(
        restarted
            .core
            .query("clone.recoveries", &json!({"project_id":1}))
            .unwrap()["attempts"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert!(restarted.tool.calls.lock().unwrap().is_empty());
    assert!(
        !restarted
            .sink
            .events
            .lock()
            .unwrap()
            .iter()
            .any(|(name, _)| name == "clone.created")
    );
}
