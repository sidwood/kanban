//! App gate for workspace observation through the shipped git observer
//! and real SQLite persistence (KAN-T31).

use std::fs;
use std::path::Path;
use std::process::Command;
use std::sync::Arc;

use kanban_app::catalog::exposed_operations;
use kanban_app::dispatch::Core;
use kanban_app::herdr::NoopHerdrProjectObserver;
use kanban_app::project::ProjectStore;
use kanban_app::workspace::WorkspaceStore;
use kanban_domain::{ProjectRegistration, WorkspaceHealth, WorkspaceId};
use kanban_dto::{TimelineEntityKind, TimelineEntityRef, TimelineEventKind};
use kanban_service::LocalRepositories;
use kanban_service::git_observer::LocalWorkspaceGitObserver;
use kanban_storage::{
    AllowAllMigrations, Database, RetentionPolicy, SqliteHerdrSettingsStore,
    SqliteIdempotencyStore, SqliteInitiativeStore, SqliteProjectStore, SqliteWorkspaceStore,
};
use serde_json::json;
use tempfile::TempDir;

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
    fs::write(dir.join("README.md"), "seed\n").expect("the seed file is written");
    git(dir, &["add", "."]);
    git(dir, &["commit", "-m", "initial"]);
    dir.to_str().expect("the path is UTF-8").to_owned()
}

fn init_repo_at(parent: &Path, name: &str) -> String {
    let dir = parent.join(name);
    fs::create_dir_all(&dir).expect("the repository directory is created");
    init_repo(&dir)
}

fn set_bc_source(workspace: &Path, source: &str) {
    git(
        workspace,
        &[
            "config",
            "bc.source",
            Path::new(source)
                .canonicalize()
                .expect("the source resolves")
                .to_str()
                .expect("the path is UTF-8"),
        ],
    );
}

fn register(workspace_id: u64, key: &str, version: u64) -> serde_json::Value {
    json!({
        "mutation": { "optimistic_version": version, "idempotency_key": key },
        "workspace_id": workspace_id,
    })
}

fn register_workspace(project_id: u64, path: &str, key: &str) -> serde_json::Value {
    json!({
        "mutation": { "optimistic_version": 0, "idempotency_key": key },
        "project_id": project_id,
        "path": path,
    })
}

struct Wired {
    core: Core,
    projects: Arc<SqliteProjectStore>,
    workspaces: Arc<SqliteWorkspaceStore>,
}

fn wired(scratch: &Path) -> Wired {
    let database_path = scratch.join("kanban.sqlite");
    let mut database = Database::open(&database_path).expect("a scratch database opens");
    database
        .migrate(&AllowAllMigrations)
        .expect("the migrations apply");
    let projects = Arc::new(SqliteProjectStore::new(&database));
    let workspaces = Arc::new(SqliteWorkspaceStore::new(&database));
    let initiatives = Arc::new(SqliteInitiativeStore::new(&database));
    let idempotency = Arc::new(SqliteIdempotencyStore::new(
        &database,
        RetentionPolicy::keep_most_recent(
            std::num::NonZeroU32::new(100).expect("the bound is not zero"),
        ),
    ));
    let mut core = Core::new(
        exposed_operations(),
        idempotency,
        Arc::new(kanban_app::events::NoopEventSink),
    );
    core.register_initiatives(initiatives.clone())
        .expect("the initiative operations register");
    core.register_projects(
        projects.clone(),
        Arc::new(LocalRepositories),
        initiatives,
        Arc::new(SqliteHerdrSettingsStore::new(&database)),
        Arc::new(NoopHerdrProjectObserver),
    )
    .expect("the project operations register");
    core.register_workspaces(
        workspaces.clone(),
        projects.clone(),
        Arc::new(LocalWorkspaceGitObserver),
    )
    .expect("the workspace operations register");
    Wired {
        core,
        projects,
        workspaces,
    }
}

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
            kanban_app::timeline::TimelineEnvelope::project(
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

#[test]
fn workspace_observe_reads_git_state_through_the_shipped_observer() {
    let scratch = TempDir::new().expect("a scratch directory is available");
    let Wired {
        core,
        projects,
        workspaces,
    } = wired(scratch.path());

    let git_dir = TempDir::new().expect("a scratch directory is available");
    let repository = init_repo(git_dir.path());
    git(
        Path::new(&repository),
        &["remote", "add", "origin", "https://example.com/kanban.git"],
    );
    let workspace = git_dir.path().join("clone");
    git(
        git_dir.path(),
        &["clone", "--local", &repository, workspace.to_str().unwrap()],
    );
    set_bc_source(&workspace, &repository);

    create_project(
        &projects,
        &repository,
        workspace.to_str().expect("the path is UTF-8"),
    );

    let head_before = Command::new("git")
        .args(["-C", workspace.to_str().unwrap(), "rev-parse", "HEAD"])
        .output()
        .expect("head reads");

    core.command(
        "workspace.register",
        &register_workspace(1, workspace.to_str().unwrap(), "key-1"),
    )
    .expect("the workspace registers");
    let response = core
        .command("workspace.observe", &register(1, "key-2", 1))
        .expect("the observation applies");

    let head_after = Command::new("git")
        .args(["-C", workspace.to_str().unwrap(), "rev-parse", "HEAD"])
        .output()
        .expect("head reads");

    assert_eq!(head_before.stdout, head_after.stdout, "HEAD must not move");
    assert_eq!(response["health"], json!("available"));
    assert_eq!(response["observation"]["branch"], json!("main"));
    assert!(response["observation"]["head"].as_str().is_some());
    assert_eq!(response["observation"]["working_tree_clean"], json!(true));

    let stored = workspaces
        .find(WorkspaceId::new(1))
        .expect("the workspace loads")
        .expect("the workspace exists");
    assert_eq!(stored.health(), WorkspaceHealth::Available);
    assert_eq!(stored.observation().branch(), Some("main"));
    assert!(stored.observation().head().is_some());
    assert_eq!(stored.observation().working_tree_clean(), Some(true));
    assert_eq!(stored.observation().unique_unlanded_commits(), Some(false));
    assert!(stored.reuse_evaluation().reusable());

    let row: (
        String,
        Option<String>,
        Option<String>,
        Option<i64>,
        Option<i64>,
    ) = rusqlite::Connection::open(scratch.path().join("kanban.sqlite"))
        .expect("the database reopens")
        .query_row(
            "SELECT health, branch, head, working_tree_clean, unique_unlanded_commits
             FROM workspaces WHERE id = 1",
            [],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            },
        )
        .expect("the SQLite row is readable");
    assert_eq!(row.0, "available");
    assert_eq!(row.1.as_deref(), Some("main"));
    assert!(row.2.is_some());
    assert_eq!(row.3, Some(1));
    assert_eq!(row.4, Some(0));
}

#[test]
fn workspace_observe_refuses_an_unrelated_same_origin_clone() {
    let scratch = TempDir::new().expect("a scratch directory is available");
    let Wired { core, projects, .. } = wired(scratch.path());

    let git_dir = TempDir::new().expect("a scratch directory is available");
    let origin = init_repo_at(git_dir.path(), "origin");
    let repository = git_dir.path().join("repository");
    let unrelated = git_dir.path().join("unrelated");
    git(
        Path::new(&origin),
        &["clone", "--local", &origin, repository.to_str().unwrap()],
    );
    git(
        Path::new(&origin),
        &["clone", "--local", &origin, unrelated.to_str().unwrap()],
    );
    create_project(
        &projects,
        repository.to_str().expect("the path is UTF-8"),
        repository.to_str().expect("the path is UTF-8"),
    );

    core.command(
        "workspace.register",
        &register_workspace(1, unrelated.to_str().expect("the path is UTF-8"), "key-1"),
    )
    .expect("the workspace registers");
    let response = core
        .command("workspace.observe", &register(1, "key-2", 1))
        .expect("the observation applies");

    assert_eq!(
        response["health"],
        json!("missing"),
        "origin URL equality alone must not establish membership"
    );
    assert_eq!(response["observation"]["repository_identity"], json!(null));
    assert_eq!(response["observation"]["branch"], json!(null));
    assert_eq!(response["observation"]["head"], json!(null));
}

struct Fixture {
    core: Core,
    workspaces: Arc<SqliteWorkspaceStore>,
    database_path: std::path::PathBuf,
}

fn fixture() -> (TempDir, Fixture) {
    let scratch = TempDir::new().expect("a scratch directory is available");
    let Wired {
        core,
        projects,
        workspaces,
    } = wired(scratch.path());
    create_project(
        &projects,
        scratch.path().to_str().expect("the path is UTF-8"),
        "/workspaces/kanban.seed",
    );
    let database_path = scratch.path().join("kanban.sqlite");
    (
        scratch,
        Fixture {
            core,
            workspaces,
            database_path,
        },
    )
}

/// A local branch clone of the fixture's repository, registered as the
/// Project's Workspace.
fn registered_clone(dir: &Path, repository: &str, core: &Core, key: &str) -> String {
    let workspace = dir.join(format!("clone-{key}"));
    git(
        Path::new(repository),
        &["clone", "--local", repository, workspace.to_str().unwrap()],
    );
    git(
        &workspace,
        &[
            "config",
            "bc.source",
            Path::new(repository)
                .canonicalize()
                .expect("the repository resolves")
                .to_str()
                .expect("the path is UTF-8"),
        ],
    );
    core.command(
        "workspace.register",
        &json!({
            "mutation": { "optimistic_version": 0, "idempotency_key": key },
            "project_id": 1,
            "path": workspace.to_str().unwrap(),
        }),
    )
    .expect("the workspace registers");
    workspace.to_str().expect("the path is UTF-8").to_owned()
}

fn observe(core: &Core, workspace_id: u64, key: &str, version: u64) -> serde_json::Value {
    core.command(
        "workspace.observe",
        &json!({
            "mutation": { "optimistic_version": version, "idempotency_key": key },
            "workspace_id": workspace_id,
        }),
    )
    .expect("the observation applies")
}

#[test]
fn unique_unlanded_commits_block_reuse_end_to_end() {
    let (dir, fixture) = fixture();
    let repository = init_repo(dir.path());
    let workspace = registered_clone(dir.path(), &repository, &fixture.core, "unlanded");
    let workspace = Path::new(&workspace);
    fs::write(workspace.join("work.md"), "local change\n").expect("the local change is written");
    git(workspace, &["add", "."]);
    git(workspace, &["commit", "-m", "local work"]);

    let response = observe(&fixture.core, 1, "key-2", 1);

    assert_eq!(response["health"], json!("available"));
    assert_eq!(
        response["observation"]["unique_unlanded_commits"],
        json!(true),
        "the shipped observer must detect commits the seed lacks"
    );
    assert_eq!(
        response["reuse"],
        json!({
            "reusable": false,
            "clean": true,
            "unassigned": true,
            "free_of_unlanded_commits": false,
        }),
        "a clean, unassigned clone with unlanded work is refused for reuse"
    );

    let stored = fixture
        .workspaces
        .find(WorkspaceId::new(1))
        .expect("the workspace loads")
        .expect("the workspace exists");
    assert_eq!(stored.observation().unique_unlanded_commits(), Some(true));
    assert!(!stored.reuse_evaluation().reusable());

    let unlanded: Option<i64> = rusqlite::Connection::open(&fixture.database_path)
        .expect("the database reopens")
        .query_row(
            "SELECT unique_unlanded_commits FROM workspaces WHERE id = 1",
            [],
            |row| row.get(0),
        )
        .expect("the SQLite row is readable");
    assert_eq!(unlanded, Some(1));
}
