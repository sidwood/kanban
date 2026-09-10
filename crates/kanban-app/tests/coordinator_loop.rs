//! App gate for the Coordinator execution loop (KAN-T46, KAN-S9-US2,
//! DR-HB-15): the Coordinator claims a Dispatch Request, selects
//! capacity, prepares a Workspace under the reuse rules, launches the
//! implementer through Herdr, and acknowledges the run. Each loop
//! step is recorded on the timeline with run and role correlation.
//! Scripted Herdr fixtures drive the loop end to end.

use std::collections::HashMap;
use std::num::NonZeroU32;
use std::sync::{Arc, Mutex, mpsc};

use kanban_app::catalog::exposed_operations;
use kanban_app::dispatch::Core;
use kanban_app::events::NoopEventSink;
use kanban_app::workspace::{WorkspaceGitObserver, WorkspaceGitSnapshot};
use kanban_app::{
    CloneGuardStore, CoordinatorHerdr, CoordinatorLoop, CoordinatorLoopRequest, CoordinatorWake,
    CoordinatorWakeRequest, ExecutionAdmission, FleetCloneTool, TimelineEnvelope,
};
use kanban_domain::{HerdrSession, ProjectId, WorkspaceCheckout, WorkspaceId};
use kanban_dto::{ApiError, CloneRecoveryRecord};
use kanban_herdr::fixture::{ScriptedSession, SessionScript};
use kanban_herdr::{HerdrRequest, PromptRequest, SessionClient, SessionMapping};
use kanban_service::LocalCloneTargetProbe;
use kanban_storage::{
    AllowAllMigrations, Database, RetentionPolicy, SqliteCapacityStore, SqliteCloneGuardStore,
    SqliteDependencyStore, SqliteDispatchStore, SqliteGraphProposalStore, SqliteIdempotencyStore,
    SqliteLaneStore, SqliteProfileStore, SqliteProjectStore, SqliteRunStore, SqliteScheduleStore,
    SqliteTicketStore, SqliteWorkspaceStore,
};
use serde_json::{Value, json};
use tempfile::TempDir;

mod common;

use common::{insert_ready_ticket, insert_ticket, mutation, seed_project_profile};

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
    step_hook: StepHook,
}

/// One callback armed against the Coordinator step whose timeline row
/// releases it.
type StepHook = Arc<Mutex<Option<(&'static str, Box<dyn FnOnce() + Send>)>>>;

/// A timeline store that releases the armed callback the instant the
/// named Coordinator step's row lands, so a test can drive a
/// competing operator command into that exact point of the loop.
struct HookedCloneGuard {
    inner: SqliteCloneGuardStore,
    hook: StepHook,
}

impl CloneGuardStore for HookedCloneGuard {
    fn append(&self, envelope: TimelineEnvelope) -> Result<(), ApiError> {
        let step = envelope.detail()["step"].as_str().map(str::to_owned);
        self.inner.append(envelope)?;
        let armed = {
            let mut hook = self.hook.lock().expect("the hook lock is sound");
            if hook
                .as_ref()
                .is_some_and(|(at, _)| step.as_deref() == Some(*at))
            {
                hook.take().map(|(_, callback)| callback)
            } else {
                None
            }
        };
        if let Some(callback) = armed {
            callback();
        }
        Ok(())
    }

    fn prepare_creation(&self, intent: &CloneRecoveryRecord) -> Result<(), ApiError> {
        self.inner.prepare_creation(intent)
    }

    fn complete_creation(&self, key: &str, workspace_id: WorkspaceId) -> Result<(), ApiError> {
        self.inner.complete_creation(key, workspace_id)
    }

    fn pending_creations(
        &self,
        project_id: ProjectId,
    ) -> Result<Vec<CloneRecoveryRecord>, ApiError> {
        self.inner.pending_creations(project_id)
    }
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
    let proposals = Arc::new(SqliteGraphProposalStore::new(&database));
    let step_hook: StepHook = Arc::new(Mutex::new(None));
    let clone_guard = Arc::new(HookedCloneGuard {
        inner: SqliteCloneGuardStore::new(&database),
        hook: step_hook.clone(),
    });
    let wake = Arc::new(RecordingWake::default());
    let idempotency = Arc::new(SqliteIdempotencyStore::new(
        &database,
        RetentionPolicy::keep_most_recent(NonZeroU32::new(100).expect("the bound is not zero")),
    ));
    let mut core = Core::new(exposed_operations(), idempotency, Arc::new(NoopEventSink));
    core.register_workspaces(workspaces.clone(), projects.clone(), git.clone())
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
        git,
    )
    .expect("the clone operations register");
    core.register_dispatch(
        requests.clone(),
        tickets.clone(),
        profiles.clone(),
        projects.clone(),
        capacity,
        lanes.clone(),
        dependencies.clone(),
        proposals.clone(),
        wake,
    )
    .expect("the dispatch operations register");
    core.register_runs(
        runs,
        requests.clone(),
        tickets.clone(),
        profiles,
        projects.clone(),
        dependencies.clone(),
        proposals.clone(),
    )
    .expect("the run operations register");
    core.register_lifecycle(
        tickets.clone(),
        dependencies.clone(),
        projects.clone(),
        Arc::new(SqliteScheduleStore::new(&database)),
        None,
    )
    .expect("the lifecycle operations register");

    let core = Arc::new(core);
    let admission = Arc::new(ExecutionAdmission::new(
        tickets.clone(),
        projects,
        dependencies,
        proposals,
    ));
    let loop_ = CoordinatorLoop::new(
        core.clone(),
        clone_guard,
        herdr,
        tickets,
        lanes,
        workspaces,
        requests,
        admission,
    );

    CoordinatorHarness {
        _dir: dir,
        core,
        loop_,
        database_path,
        step_hook,
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
fn coordinator_loop_refuses_a_fresh_clone_on_the_wrong_branch() {
    let git = Arc::new(ScriptedGit {
        snapshots: HashMap::from([(
            "/workspaces/kanban.kan-t1".to_owned(),
            WorkspaceGitSnapshot {
                present: true,
                repository_identity: Some("identity".to_owned()),
                checkout: Some(WorkspaceCheckout::Branch("wrong-branch".to_owned())),
                head: Some("a".repeat(40)),
                working_tree_clean: Some(true),
                unique_unlanded_commits: Some(false),
            },
        )]),
    });
    let herdr = Arc::new(RecordingHerdr {
        accepted: true,
        ..RecordingHerdr::default()
    });
    let harness = coordinator_harness(git, herdr.clone());
    let ticket = insert_ready_ticket(&harness.database_path, 1, "normal");
    let request = enqueue(&harness.core, ticket, "wrong-branch");
    assert!(
        harness
            .loop_
            .execute(CoordinatorLoopRequest {
                project_id: 1,
                dispatch_request_id: request
            })
            .is_err()
    );
    assert!(herdr.launches.lock().unwrap().is_empty());
}

#[test]
fn coordinator_loop_claims_prepares_launches_and_acknowledges() {
    let git = clean_git();
    let herdr = Arc::new(RecordingHerdr {
        accepted: true,
        ..RecordingHerdr::default()
    });
    let harness = coordinator_harness(git, herdr.clone());
    let ticket = insert_ready_ticket(&harness.database_path, 1, "normal");
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
    let ticket = insert_ready_ticket(&harness.database_path, 2, "high");
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
    let ticket = insert_ready_ticket(&harness.database_path, 5, "normal");
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
        snapshots: HashMap::from([
            (
                "/workspaces/kanban.seed".to_owned(),
                WorkspaceGitSnapshot {
                    present: true,
                    repository_identity: Some("identity".to_owned()),
                    checkout: Some(WorkspaceCheckout::Branch("main".to_owned())),
                    head: Some("abc123".to_owned()),
                    working_tree_clean: Some(true),
                    unique_unlanded_commits: Some(false),
                },
            ),
            (
                "/workspaces/kanban.kan-t4".to_owned(),
                WorkspaceGitSnapshot {
                    present: true,
                    repository_identity: Some("identity".to_owned()),
                    checkout: Some(WorkspaceCheckout::Branch("kan-t4".to_owned())),
                    head: Some("abc123".to_owned()),
                    working_tree_clean: Some(true),
                    unique_unlanded_commits: Some(false),
                },
            ),
        ]),
    });
    let herdr = Arc::new(RecordingHerdr {
        accepted: true,
        ..RecordingHerdr::default()
    });
    let harness = coordinator_harness(git, herdr);
    let ticket = insert_ready_ticket(&harness.database_path, 4, "normal");
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
    let ticket = insert_ready_ticket(&harness.database_path, 3, "normal");
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

/// KAN-T138-AC2: the loop answers the shared admission invariant
/// before it seats a Lane, so a refusal leaves the Lane table and
/// the timeline exactly as they stood and launches nothing.
#[test]
fn coordinator_loop_refuses_an_ineligible_ticket_before_seating_a_lane() {
    let git = clean_git();
    let herdr = Arc::new(RecordingHerdr {
        accepted: true,
        ..RecordingHerdr::default()
    });
    let harness = coordinator_harness(git, herdr.clone());
    let ticket = insert_ticket(&harness.database_path, 1, "normal");
    let request_id = enqueue(&harness.core, ticket, "ineligible-create");

    let error = harness
        .loop_
        .execute(CoordinatorLoopRequest {
            project_id: 1,
            dispatch_request_id: request_id,
        })
        .expect_err("a draft Task is not executable");

    assert_eq!(
        error.message,
        "a draft Ticket is not executable; an implementer run admits a ready or active Ticket"
    );
    assert!(
        herdr
            .launches
            .lock()
            .expect("the launch log is sound")
            .is_empty(),
        "nothing launches"
    );
    let conn = rusqlite::Connection::open(&harness.database_path).expect("the database reopens");
    let lanes: i64 = conn
        .query_row("SELECT COUNT(*) FROM lanes", [], |row| row.get(0))
        .expect("the count serves");
    assert_eq!(lanes, 0, "no Lane is seated for a refused Ticket");
    assert!(
        coordinator_steps(&harness.database_path).is_empty(),
        "no Coordinator step is recorded"
    );
    let status: String = conn
        .query_row(
            "SELECT status FROM dispatch_requests WHERE id = ?1",
            rusqlite::params![request_id as i64],
            |row| row.get(0),
        )
        .expect("the request row serves");
    assert_eq!(status, "queued");
}

/// Every authoritative record one Coordinator pass may write: the
/// Lanes, the Dispatch Requests, the Capabilities, and the Runs a
/// refusal must leave exactly as it found them (KAN-T138-AC2).
#[derive(Debug, PartialEq, Eq)]
struct Reserved {
    lanes: Vec<(i64, Option<i64>, Option<i64>, i64)>,
    requests: Vec<(i64, String, i64)>,
    capabilities: Vec<(i64, String)>,
    runs: i64,
}

fn reserved(database_path: &std::path::Path) -> Reserved {
    let conn = rusqlite::Connection::open(database_path).expect("the database reopens");
    let lanes = conn
        .prepare("SELECT id, ticket_id, workspace_id, version FROM lanes ORDER BY id")
        .expect("the statement prepares")
        .query_map([], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })
        .expect("the rows serve")
        .collect::<Result<Vec<_>, _>>()
        .expect("the rows decode");
    let requests = conn
        .prepare("SELECT id, status, version FROM dispatch_requests ORDER BY id")
        .expect("the statement prepares")
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .expect("the rows serve")
        .collect::<Result<Vec<_>, _>>()
        .expect("the rows decode");
    let capabilities = conn
        .prepare("SELECT id, status FROM capabilities ORDER BY id")
        .expect("the statement prepares")
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .expect("the rows serve")
        .collect::<Result<Vec<_>, _>>()
        .expect("the rows decode");
    let runs: i64 = conn
        .query_row("SELECT COUNT(*) FROM runs", [], |row| row.get(0))
        .expect("the count serves");
    Reserved {
        lanes,
        requests,
        capabilities,
        runs,
    }
}

/// The timeline as the ordered labels these races read: `step:<name>`
/// for a Coordinator step and `ticket:<action>` for a Ticket
/// transition.
fn timeline_labels(database_path: &std::path::Path) -> Vec<String> {
    let conn = rusqlite::Connection::open(database_path).expect("the database reopens");
    conn.prepare(
        "SELECT entity_kind, json_extract(detail, '$.action'), json_extract(detail, '$.step')
         FROM timeline_events ORDER BY id",
    )
    .expect("the statement prepares")
    .query_map([], |row| {
        Ok((
            row.get::<_, Option<String>>(0)?,
            row.get::<_, Option<String>>(1)?,
            row.get::<_, Option<String>>(2)?,
        ))
    })
    .expect("the rows serve")
    .collect::<Result<Vec<_>, _>>()
    .expect("the rows decode")
    .into_iter()
    .filter_map(
        |(entity, action, step)| match (entity.as_deref(), action.as_deref(), step) {
            (_, Some("coordinator_step"), Some(step)) => Some(format!("step:{step}")),
            (Some("ticket"), Some(action), _) => Some(format!("ticket:{action}")),
            _ => None,
        },
    )
    .collect()
}

fn ticket_version(database_path: &std::path::Path, ticket: u64) -> u64 {
    rusqlite::Connection::open(database_path)
        .expect("the database reopens")
        .query_row(
            "SELECT version FROM tickets WHERE id = ?1",
            rusqlite::params![ticket as i64],
            |row| row.get::<_, i64>(0),
        )
        .expect("the Ticket row serves")
        .try_into()
        .expect("the version fits")
}

/// Drive `ticket.cancel` through the production command from a second
/// thread, reporting when the command is about to enter the core. The
/// competing operator command is a real one on its own thread, so the
/// loop must order itself against it rather than against a re-entrant
/// call on its own.
fn competing_cancel(
    core: Arc<Core>,
    ticket: u64,
    version: u64,
) -> (
    mpsc::Receiver<()>,
    std::thread::JoinHandle<Result<Value, ApiError>>,
) {
    let (started, waiting) = mpsc::channel();
    let handle = std::thread::spawn(move || {
        started.send(()).expect("the test still listens");
        core.command(
            "ticket.cancel",
            &json!({
                "mutation": mutation(version, "competing-cancel"),
                "ticket_id": ticket,
            }),
        )
    });
    (waiting, handle)
}

/// A Coordinator pass that meets a competing cancellation has exactly
/// two legal outcomes and nothing between them: it was refused and
/// left every authoritative record as it stood, launching nothing, or
/// it was admitted and launched once for a Run it went on to
/// acknowledge (KAN-T138-AC1, KAN-T138-AC2).
fn assert_no_orphaned_reservation(
    outcome: &Result<kanban_app::CoordinatorLoopOutcome, ApiError>,
    before: &Reserved,
    database_path: &std::path::Path,
    launches: &[kanban_app::ImplementerLaunch],
) {
    let after = reserved(database_path);
    match outcome {
        Err(_) => {
            assert!(
                launches.is_empty(),
                "a refused pass launches nothing: {launches:?}"
            );
            assert_eq!(
                &after, before,
                "a refused pass leaves the Lane, the request, the Capability, and capacity as they stood"
            );
        }
        Ok(admitted) => {
            assert_eq!(
                launches.len(),
                1,
                "an admitted pass launches exactly once: {launches:?}"
            );
            assert_eq!(launches[0].lane_id, admitted.lane_id);
            assert_eq!(after.runs, before.runs + 1, "the launch has its Run");
            let labels = timeline_labels(database_path);
            let launched = labels
                .iter()
                .position(|label| label == "step:launch")
                .expect("the admitted pass recorded its launch");
            let cancelled = labels
                .iter()
                .position(|label| label == "ticket:cancelled")
                .expect("the competing cancellation landed");
            assert!(
                cancelled > launched,
                "an admitted launch is authorised before the cancellation commits: {labels:?}"
            );
        }
    }
}

/// KAN-T138-AC1, KAN-T138-AC2 (reviewer race probe, cancellation
/// before the claim): a cancellation competing with the Coordinator
/// between its admission and its claim never leaves a Lane seated for
/// work that never ran.
#[test]
fn coordinator_loop_never_seats_a_lane_for_work_a_competing_cancellation_refuses() {
    assert_cancellation_races_the_loop("seat_lane");
}

/// KAN-T138-AC1, KAN-T138-AC2 (reviewer race probe, cancellation
/// after Workspace preparation): a cancellation competing with the
/// Coordinator after its Workspace assignment never produces a launch
/// the loop then refuses to acknowledge.
#[test]
fn coordinator_loop_never_launches_work_a_competing_cancellation_refuses() {
    assert_cancellation_races_the_loop("assign_workspace");
}

fn assert_cancellation_races_the_loop(step: &'static str) {
    let herdr = Arc::new(RecordingHerdr {
        accepted: true,
        ..RecordingHerdr::default()
    });
    let harness = coordinator_harness(clean_git(), herdr.clone());
    let ticket = insert_ready_ticket(&harness.database_path, 1, "normal");
    let request = enqueue(&harness.core, ticket, "race-enqueue");
    let before = reserved(&harness.database_path);
    let version = ticket_version(&harness.database_path, ticket);

    let canceller = Arc::new(Mutex::new(None));
    let slot = canceller.clone();
    let core = harness.core.clone();
    *harness.step_hook.lock().expect("the hook lock is sound") = Some((
        step,
        Box::new(move || {
            let (started, handle) = competing_cancel(core, ticket, version);
            started
                .recv()
                .expect("the competing cancel reaches the core");
            std::thread::sleep(std::time::Duration::from_millis(250));
            *slot.lock().expect("the canceller slot is sound") = Some(handle);
        }),
    ));

    let outcome = harness.loop_.execute(CoordinatorLoopRequest {
        project_id: 1,
        dispatch_request_id: request,
    });
    let handle = canceller
        .lock()
        .expect("the canceller slot is sound")
        .take()
        .expect("the hook fired at its step");
    handle
        .join()
        .expect("the competing thread finishes")
        .expect("the operator's own cancellation commits");

    // The reservation admits no interleaving, so the operator's
    // cancellation lands after the pass rather than inside it.
    assert!(
        outcome.is_ok(),
        "{step}: the pass ran to completion beside its competing cancellation: {outcome:?}"
    );
    let launches = herdr.launches.lock().expect("the launch log is sound");
    assert_no_orphaned_reservation(&outcome, &before, &harness.database_path, &launches);
}

/// KAN-T138-AC1, KAN-T138-AC2: work made ineligible before the
/// Coordinator wakes is refused with nothing reserved — no Lane, no
/// claim, no Capability, no capacity, and no launch — whichever fact
/// made it ineligible.
#[test]
fn coordinator_loop_reserves_nothing_for_work_made_ineligible_before_the_pass() {
    for invalidation in ["cancelled", "archived", "blocked"] {
        let herdr = Arc::new(RecordingHerdr {
            accepted: true,
            ..RecordingHerdr::default()
        });
        let harness = coordinator_harness(clean_git(), herdr.clone());
        let ticket = insert_ready_ticket(&harness.database_path, 1, "normal");
        let request = enqueue(&harness.core, ticket, "ineligible-enqueue");
        match invalidation {
            "cancelled" => {
                harness
                    .core
                    .command(
                        "ticket.cancel",
                        &json!({
                            "mutation": mutation(
                                ticket_version(&harness.database_path, ticket),
                                "ineligible-cancel",
                            ),
                            "ticket_id": ticket,
                        }),
                    )
                    .expect("the operator's cancellation commits");
            }
            "archived" => {
                rusqlite::Connection::open(&harness.database_path)
                    .expect("the database reopens")
                    .execute("UPDATE projects SET archived = 1 WHERE id = 1", [])
                    .expect("the fixture Project archives");
            }
            _ => common::insert_blocker(&harness.database_path, ticket),
        }
        let before = reserved(&harness.database_path);

        let outcome = harness.loop_.execute(CoordinatorLoopRequest {
            project_id: 1,
            dispatch_request_id: request,
        });

        assert!(outcome.is_err(), "{invalidation}: the pass is refused");
        assert_eq!(
            reserved(&harness.database_path),
            before,
            "{invalidation}: the refused pass reserves nothing"
        );
        assert!(
            herdr
                .launches
                .lock()
                .expect("the launch log is sound")
                .is_empty(),
            "{invalidation}: nothing launches"
        );
        assert!(
            coordinator_steps(&harness.database_path).is_empty(),
            "{invalidation}: no Coordinator step is recorded"
        );
    }
}
