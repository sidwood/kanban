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

#[test]
fn workspace_observe_reads_git_state_through_the_shipped_observer() {
    let scratch = TempDir::new().expect("a scratch directory is available");
    let database_path = scratch.path().join("kanban.sqlite");
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
    git(
        &workspace,
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

    let registration = ProjectRegistration::new(
        "CORE",
        "Control plane",
        &repository,
        workspace.to_str().unwrap(),
        "main",
        "kanban.seed",
        Some("kanban-main"),
        None,
    )
    .expect("the fixture registration validates");
    projects
        .create(&registration, &|id| {
            kanban_app::timeline::TimelineEnvelope::project(
                &id.value().to_string(),
                TimelineEventKind::Transition,
                Some(TimelineEntityRef {
                    kind: TimelineEntityKind::Project,
                    id: id.value().to_string(),
                }),
                json!({ "action": "registered", "code": "CORE", "id": id.value() }),
            )
            .expect("a minted Project identity names a Project")
        })
        .expect("the project registers");

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

    let row: (String, Option<String>, Option<String>, Option<i64>) =
        rusqlite::Connection::open(&database_path)
            .expect("the database reopens")
            .query_row(
                "SELECT health, branch, head, working_tree_clean FROM workspaces WHERE id = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .expect("the SQLite row is readable");
    assert_eq!(row.0, "available");
    assert_eq!(row.1.as_deref(), Some("main"));
    assert!(row.2.is_some());
    assert_eq!(row.3, Some(1));
}
