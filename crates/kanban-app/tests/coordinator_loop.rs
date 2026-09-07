//! App gate for the Coordinator execution loop (KAN-T46, KAN-S9-US2,
//! DR-HB-15): the Coordinator claims a Dispatch Request, selects
//! capacity, prepares a Workspace under the reuse rules, launches the
//! implementer through Herdr, and acknowledges the run. Each loop
//! step is recorded on the timeline with run and role correlation.
//! Scripted Herdr fixtures drive the loop end to end.

use std::collections::HashMap;
use std::num::NonZeroU32;
use std::sync::{Arc, Mutex};

use kanban_app::catalog::exposed_operations;
use kanban_app::dispatch::Core;
use kanban_app::events::NoopEventSink;
use kanban_app::workspace::{WorkspaceGitObserver, WorkspaceGitSnapshot};
use kanban_app::{
    CoordinatorHerdr, CoordinatorLoop, CoordinatorLoopRequest, CoordinatorWake,
    CoordinatorWakeRequest, FleetCloneTool,
};
use kanban_domain::{HerdrSession, WorkspaceCheckout};
use kanban_dto::ApiError;
use kanban_herdr::fixture::{ScriptedSession, SessionScript};
use kanban_herdr::{HerdrRequest, PromptRequest, SessionClient, SessionMapping};
use kanban_service::LocalCloneTargetProbe;
use kanban_storage::{
    AllowAllMigrations, Database, RetentionPolicy, SqliteCapacityStore, SqliteCloneGuardStore,
    SqliteDependencyStore, SqliteDispatchStore, SqliteIdempotencyStore, SqliteLaneStore,
    SqliteProfileStore, SqliteProjectStore, SqliteRunStore, SqliteTicketStore,
    SqliteWorkspaceStore,
};
use serde_json::json;
use tempfile::TempDir;

mod common;

use common::{insert_ticket, mutation, seed_project_profile};

#[derive(Default)]
struct RecordingWake {
    calls: Mutex<Vec<CoordinatorWakeRequest>>,
}

impl CoordinatorWake for RecordingWake {
    fn wake(&self, request: CoordinatorWakeRequest) {
        self.calls
            .lock()
            .expect("the wake log is sound")
            .push(request);
    }
}

#[derive(Default)]
struct RecordingHerdr {
    launches: Mutex<Vec<kanban_app::ImplementerLaunch>>,
    accepted: bool,
}

impl CoordinatorHerdr for RecordingHerdr {
    fn launch_implementer(&self, request: kanban_app::ImplementerLaunch) -> Result<bool, ApiError> {
        self.launches
            .lock()
            .expect("the launch log is sound")
            .push(request);
        Ok(self.accepted)
    }
}

#[derive(Default)]
struct RecordingCloneTool {
    calls: Mutex<Vec<(String, String, String)>>,
}

impl FleetCloneTool for RecordingCloneTool {
    fn add_clone(&self, source: &str, path: &str, branch: &str) -> Result<(), ApiError> {
        self.calls.lock().expect("the clone log is sound").push((
            source.to_owned(),
            path.to_owned(),
            branch.to_owned(),
        ));
        Ok(())
    }

    fn remove_clone(&self, _path: &str) -> Result<(), ApiError> {
        Ok(())
    }
}

#[derive(Default)]
struct AcceptingCloneTool;

impl FleetCloneTool for AcceptingCloneTool {
    fn add_clone(&self, _source: &str, _path: &str, _branch: &str) -> Result<(), ApiError> {
        Ok(())
    }

    fn remove_clone(&self, _path: &str) -> Result<(), ApiError> {
        Ok(())
    }
}

struct ScriptedGit {
    snapshots: HashMap<String, WorkspaceGitSnapshot>,
}

impl WorkspaceGitObserver for ScriptedGit {
    fn observe(&self, workspace_path: &str, _repository_path: &str) -> WorkspaceGitSnapshot {
        self.snapshots
            .get(workspace_path)
            .cloned()
            .unwrap_or_else(|| WorkspaceGitSnapshot {
                present: true,
                repository_identity: Some("identity".to_owned()),
                checkout: Some(WorkspaceCheckout::Branch("kan-t1".to_owned())),
                head: Some("abc123".to_owned()),
                working_tree_clean: Some(true),
                unique_unlanded_commits: Some(false),
            })
    }
}

struct CoordinatorHarness {
    _dir: TempDir,
    core: Arc<Core>,
    loop_: CoordinatorLoop,
    database_path: std::path::PathBuf,
}

fn coordinator_harness(
    git: Arc<ScriptedGit>,
    herdr: Arc<dyn CoordinatorHerdr>,
) -> CoordinatorHarness {
    coordinator_harness_with_clone_tool(git, herdr, Arc::new(AcceptingCloneTool))
}

fn coordinator_harness_with_clone_tool(
    git: Arc<ScriptedGit>,
    herdr: Arc<dyn CoordinatorHerdr>,
    clone_tool: Arc<dyn FleetCloneTool>,
) -> CoordinatorHarness {
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
    let runs = Arc::new(SqliteRunStore::new(&database));
    let workspaces = Arc::new(SqliteWorkspaceStore::new(&database));
    let clone_guard = Arc::new(SqliteCloneGuardStore::new(&database));
    let wake = Arc::new(RecordingWake::default());
    let idempotency = Arc::new(SqliteIdempotencyStore::new(
        &database,
        RetentionPolicy::keep_most_recent(NonZeroU32::new(100).expect("the bound is not zero")),
    ));
    let mut core = Core::new(exposed_operations(), idempotency, Arc::new(NoopEventSink));
    core.register_workspaces(workspaces.clone(), projects.clone(), git)
        .expect("the workspace operations register");
    core.register_lanes(
        lanes.clone(),
        projects.clone(),
        workspaces.clone(),
        tickets.clone(),
    )
    .expect("the lane operations register");
    core.register_clones(
        clone_tool,
        projects.clone(),
        workspaces.clone(),
        clone_guard.clone(),
        Arc::new(LocalCloneTargetProbe),
    )
    .expect("the clone operations register");
    core.register_dispatch(
        requests.clone(),
        tickets.clone(),
        profiles.clone(),
        projects.clone(),
        capacity,
        lanes.clone(),
        dependencies,
        wake,
    )
    .expect("the dispatch operations register");
    core.register_runs(
        runs,
        requests.clone(),
        tickets.clone(),
        profiles,
        projects.clone(),
    )
    .expect("the run operations register");

    let core = Arc::new(core);
    let loop_ = CoordinatorLoop::new(
        core.clone(),
        clone_guard,
        herdr,
        tickets,
        lanes,
        workspaces,
        requests,
    );

    CoordinatorHarness {
        _dir: dir,
        core,
        loop_,
        database_path,
    }
}

fn clean_git() -> Arc<ScriptedGit> {
    Arc::new(ScriptedGit {
        snapshots: HashMap::new(),
    })
}

fn enqueue(core: &Arc<Core>, ticket: u64, key: &str) -> u64 {
    let created = core
        .command(
            "dispatch.request",
            &json!({
                "mutation": mutation(0, key),
                "ticket_id": ticket,
            }),
        )
        .expect("the request is created");
    created["id"].as_u64().expect("the identity is a number")
}

type CoordinatorCorrelation = (Option<String>, Option<String>, Option<i64>, String, String);

fn coordinator_steps(database_path: &std::path::Path) -> Vec<String> {
    let conn = rusqlite::Connection::open(database_path).expect("the database reopens");
    conn.prepare(
        "SELECT json_extract(detail, '$.step') FROM timeline_events
         WHERE json_extract(detail, '$.action') = 'coordinator_step'
         ORDER BY id",
    )
    .expect("the statement prepares")
    .query_map([], |row| row.get(0))
    .expect("the rows serve")
    .collect::<Result<Vec<_>, _>>()
    .expect("the steps decode")
}

#[test]
fn coordinator_loop_claims_prepares_launches_and_acknowledges() {
    let git = clean_git();
    let herdr = Arc::new(RecordingHerdr {
        accepted: true,
        ..RecordingHerdr::default()
    });
    let harness = coordinator_harness(git, herdr.clone());
    let ticket = insert_ticket(&harness.database_path, 1, "normal");
    let request_id = enqueue(&harness.core, ticket, "loop-create");

    let outcome = harness
        .loop_
        .execute(CoordinatorLoopRequest {
            project_id: 1,
            dispatch_request_id: request_id,
        })
        .expect("the Coordinator loop completes");

    assert!(outcome.claimed);
    assert!(outcome.run_id > 0);
    assert!(outcome.lane_id > 0);
    assert!(outcome.workspace_id > 0);

    let launches = herdr.launches.lock().expect("the launch log is sound");
    assert_eq!(launches.len(), 1);
    assert_eq!(launches[0].ticket_id, ticket);
    assert_eq!(launches[0].dispatch_request_id, request_id);

    assert_eq!(
        coordinator_steps(&harness.database_path),
        vec![
            "seat_lane".to_owned(),
            "claim".to_owned(),
            "prepare_workspace".to_owned(),
            "assign_workspace".to_owned(),
            "launch".to_owned(),
            "acknowledge".to_owned(),
        ]
    );

    let conn = rusqlite::Connection::open(&harness.database_path).expect("the database reopens");
    let correlated: Vec<CoordinatorCorrelation> = conn
        .prepare(
            "SELECT entity_kind, entity_id, json_extract(detail, '$.run_id'), json_extract(detail, '$.role'), json_extract(detail, '$.step')
             FROM timeline_events
             WHERE json_extract(detail, '$.action') = 'coordinator_step'
             ORDER BY id",
        )
        .expect("the statement prepares")
        .query_map([], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
            ))
        })
        .expect("the rows serve")
        .collect::<Result<Vec<_>, _>>()
        .expect("the correlation decodes");
    assert!(
        correlated
            .iter()
            .filter(|(_, _, _, _, step)| *step != "launch" && *step != "acknowledge")
            .all(|(kind, _, _, role, _)| kind.as_deref() == Some("ticket") && role == "coordinator"),
        "pre-acknowledge steps correlate to the Ticket and name the Coordinator role"
    );
    let launch = correlated
        .iter()
        .find(|(_, _, _, _, step)| *step == "launch")
        .expect("the launch step is recorded");
    assert_eq!(launch.0.as_deref(), Some("ticket"));
    assert_eq!(launch.3, "implementer");
    let acknowledge = correlated
        .iter()
        .find(|(_, _, _, _, step)| *step == "acknowledge")
        .expect("the acknowledge step is recorded");
    assert_eq!(acknowledge.0.as_deref(), Some("run"));
    assert_eq!(
        acknowledge.2.map(|run| run as u64),
        Some(outcome.run_id),
        "the acknowledge step carries the run identity"
    );
}

#[test]
fn coordinator_loop_reuses_a_clean_workspace_under_the_reuse_rules() {
    let clone_tool = Arc::new(RecordingCloneTool::default());
    let git = Arc::new(ScriptedGit {
        snapshots: HashMap::from([(
            "/workspaces/kanban.kan-t2".to_owned(),
            WorkspaceGitSnapshot {
                present: true,
                repository_identity: Some("identity".to_owned()),
                checkout: Some(WorkspaceCheckout::Branch("kan-t2".to_owned())),
                head: Some("abc123".to_owned()),
                working_tree_clean: Some(true),
                unique_unlanded_commits: Some(false),
            },
        )]),
    });
    let herdr = Arc::new(RecordingHerdr {
        accepted: true,
        ..RecordingHerdr::default()
    });
    let harness = coordinator_harness_with_clone_tool(git, herdr, clone_tool.clone());
    let ticket = insert_ticket(&harness.database_path, 2, "high");
    let request_id = enqueue(&harness.core, ticket, "reuse-create");

    let workspace = harness
        .core
        .command(
            "workspace.register",
            &json!({
                "mutation": mutation(0, "reuse-register"),
                "project_id": 1,
                "path": "/workspaces/kanban.kan-t2",
            }),
        )
        .expect("the workspace registers");
    let workspace_id = workspace["id"].as_u64().expect("the identity is a number");
    harness
        .core
        .command(
            "workspace.observe",
            &json!({
                "mutation": mutation(1, "reuse-observe"),
                "workspace_id": workspace_id,
            }),
        )
        .expect("the workspace is observed");

    let outcome = harness
        .loop_
        .execute(CoordinatorLoopRequest {
            project_id: 1,
            dispatch_request_id: request_id,
        })
        .expect("the Coordinator loop completes");

    assert_eq!(outcome.workspace_id, workspace_id);

    let clone_calls = clone_tool.calls.lock().expect("the clone log is sound");
    assert!(
        clone_calls.is_empty(),
        "reuse must not invoke the fleet clone skill outside the guarded create path"
    );

    let conn = rusqlite::Connection::open(&harness.database_path).expect("the database reopens");
    let reused: bool = conn
        .query_row(
            "SELECT json_extract(detail, '$.reused') FROM timeline_events
             WHERE json_extract(detail, '$.action') = 'coordinator_step'
               AND json_extract(detail, '$.step') = 'prepare_workspace'",
            [],
            |row| row.get(0),
        )
        .expect("the prepare step is recorded");
    assert!(reused);
}

#[test]
fn coordinator_loop_refuses_reuse_when_observed_branch_mismatches_execution_branch() {
    let git = Arc::new(ScriptedGit {
        snapshots: HashMap::from([(
            "/workspaces/kanban.kan-t5".to_owned(),
            WorkspaceGitSnapshot {
                present: true,
                repository_identity: Some("identity".to_owned()),
                checkout: Some(WorkspaceCheckout::Branch("main".to_owned())),
                head: Some("abc123".to_owned()),
                working_tree_clean: Some(true),
                unique_unlanded_commits: Some(false),
            },
        )]),
    });
    let herdr = Arc::new(RecordingHerdr {
        accepted: true,
        ..RecordingHerdr::default()
    });
    let harness = coordinator_harness(git, herdr);
    let ticket = insert_ticket(&harness.database_path, 5, "normal");
    let request_id = enqueue(&harness.core, ticket, "branch-mismatch-create");

    let workspace = harness
        .core
        .command(
            "workspace.register",
            &json!({
                "mutation": mutation(0, "branch-mismatch-register"),
                "project_id": 1,
                "path": "/workspaces/kanban.kan-t5",
            }),
        )
        .expect("the workspace registers");
    let workspace_id = workspace["id"].as_u64().expect("the identity is a number");
    harness
        .core
        .command(
            "workspace.observe",
            &json!({
                "mutation": mutation(1, "branch-mismatch-observe"),
                "workspace_id": workspace_id,
            }),
        )
        .expect("the workspace is observed");

    let error = harness
        .loop_
        .execute(CoordinatorLoopRequest {
            project_id: 1,
            dispatch_request_id: request_id,
        })
        .expect_err("the Coordinator loop refuses a branch mismatch");

    assert_eq!(
        error.message,
        "the reused Workspace checkout must match the execution branch"
    );
}

#[test]
fn coordinator_loop_skips_the_seed_workspace_when_selecting_reuse_capacity() {
    let git = Arc::new(ScriptedGit {
        snapshots: HashMap::from([(
            "/workspaces/kanban.seed".to_owned(),
            WorkspaceGitSnapshot {
                present: true,
                repository_identity: Some("identity".to_owned()),
                checkout: Some(WorkspaceCheckout::Branch("main".to_owned())),
                head: Some("abc123".to_owned()),
                working_tree_clean: Some(true),
                unique_unlanded_commits: Some(false),
            },
        )]),
    });
    let herdr = Arc::new(RecordingHerdr {
        accepted: true,
        ..RecordingHerdr::default()
    });
    let harness = coordinator_harness(git, herdr);
    let ticket = insert_ticket(&harness.database_path, 4, "normal");
    let request_id = enqueue(&harness.core, ticket, "seed-skip-create");

    let seed = harness
        .core
        .command(
            "workspace.register",
            &json!({
                "mutation": mutation(0, "seed-register"),
                "project_id": 1,
                "path": "/workspaces/kanban.seed",
            }),
        )
        .expect("the seed workspace registers");
    harness
        .core
        .command(
            "workspace.observe",
            &json!({
                "mutation": mutation(1, "seed-observe"),
                "workspace_id": seed["id"].as_u64().expect("the identity is a number"),
            }),
        )
        .expect("the seed workspace is observed");

    let outcome = harness
        .loop_
        .execute(CoordinatorLoopRequest {
            project_id: 1,
            dispatch_request_id: request_id,
        })
        .expect("the Coordinator loop completes");

    assert_ne!(
        outcome.workspace_id,
        seed["id"].as_u64().expect("the identity is a number")
    );
}

#[test]
fn coordinator_loop_launches_through_the_herdr_session_socket() {
    let dir = TempDir::new().expect("a scratch directory is available");
    let fixture = ScriptedSession::bind(
        dir.path(),
        "kanban-main",
        "/workspaces/kanban.seed",
        SessionScript::default().with_prompt_accepted(true),
    );
    let mapping = SessionMapping::new(
        HerdrSession::named("kanban-main").expect("the name validates"),
        "/workspaces/kanban.seed",
        "kanban.seed",
    );
    let client = Arc::new(Mutex::new(
        SessionClient::connect(mapping, dir.path())
            .expect("the session connects through its socket"),
    ));
    struct SessionHerdr {
        client: Arc<Mutex<SessionClient>>,
    }
    impl CoordinatorHerdr for SessionHerdr {
        fn launch_implementer(
            &self,
            request: kanban_app::ImplementerLaunch,
        ) -> Result<bool, ApiError> {
            self.client
                .lock()
                .expect("the client lock is sound")
                .prompt(PromptRequest {
                    role: "implementer".to_owned(),
                    message: request.message,
                })
                .map_err(|error| ApiError::internal(&error.to_string()))
        }
    }

    let git = Arc::new(ScriptedGit {
        snapshots: HashMap::from([(
            "/workspaces/kanban.kan-t3".to_owned(),
            WorkspaceGitSnapshot {
                present: true,
                repository_identity: Some("identity".to_owned()),
                checkout: Some(WorkspaceCheckout::Branch("kan-t3".to_owned())),
                head: Some("abc123".to_owned()),
                working_tree_clean: Some(true),
                unique_unlanded_commits: Some(false),
            },
        )]),
    });
    let harness = coordinator_harness(git, Arc::new(SessionHerdr { client }));
    let ticket = insert_ticket(&harness.database_path, 3, "normal");
    let request_id = enqueue(&harness.core, ticket, "herdr-create");
    let workspace = harness
        .core
        .command(
            "workspace.register",
            &json!({
                "mutation": mutation(0, "herdr-register"),
                "project_id": 1,
                "path": "/workspaces/kanban.kan-t3",
            }),
        )
        .expect("the workspace registers");
    harness
        .core
        .command(
            "workspace.observe",
            &json!({
                "mutation": mutation(1, "herdr-observe"),
                "workspace_id": workspace["id"].as_u64().expect("the identity is a number"),
            }),
        )
        .expect("the workspace is observed");

    harness
        .loop_
        .execute(CoordinatorLoopRequest {
            project_id: 1,
            dispatch_request_id: request_id,
        })
        .expect("the Coordinator loop completes");

    let recorded = fixture.recorded_requests();
    assert!(
        recorded.iter().any(|request| {
            matches!(
                request,
                HerdrRequest::Prompt { role, .. } if role == "implementer"
            )
        }),
        "the implementer launch crosses the Herdr session socket"
    );
    assert!(
        !recorded
            .iter()
            .any(|request| matches!(request, HerdrRequest::Wake { .. })),
        "the loop does not wake itself"
    );
}
