//! App gate for standalone Bug landing (KAN-T52): a Bug with no
//! active Spec may land through the Seed; an attached Spec is refused.

mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use kanban_app::catalog::exposed_operations;
use kanban_app::dispatch::Core;
use kanban_app::herdr::NoopHerdrProjectObserver;
use kanban_app::project::ProjectStore;
use kanban_domain::ProjectRegistration;
use kanban_dto::{ErrorCode, TimelineEntityKind, TimelineEntityRef, TimelineEventKind};
use kanban_service::LocalRepositories;
use kanban_service::git_observer::LocalWorkspaceGitObserver;
use kanban_storage::{
    AllowAllMigrations, Database, RetentionPolicy, SqliteHerdrSettingsStore,
    SqliteIdempotencyStore, SqliteInitiativeStore, SqliteLaneStore, SqlitePlanStore,
    SqliteProjectStore, SqliteSpecStore, SqliteTicketStore, SqliteWorkspaceStore,
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

fn git_init(dir: &Path) {
    let status = Command::new("git")
        .env("GIT_CONFIG_COUNT", "1")
        .env("GIT_CONFIG_KEY_0", "init.defaultBranch")
        .env("GIT_CONFIG_VALUE_0", "master")
        .args(["init", "-b", "main"])
        .current_dir(dir)
        .status()
        .expect("git runs");
    assert!(status.success(), "git init -b main in {}", dir.display());
}

fn init_repo(dir: &Path) -> PathBuf {
    fs::create_dir_all(dir).expect("the repository directory is created");
    git_init(dir);
    git(dir, &["config", "user.email", "test@example.com"]);
    git(dir, &["config", "user.name", "Test"]);
    fs::write(dir.join("README.md"), "seed\n").expect("the seed file is written");
    git(dir, &["add", "."]);
    git(dir, &["commit", "-m", "initial"]);
    dir.to_path_buf()
}

fn mutation(version: u64, key: &str) -> serde_json::Value {
    json!({
        "optimistic_version": version,
        "idempotency_key": key,
    })
}

struct Wired {
    core: Core,
    _dir: TempDir,
    seed: PathBuf,
    bug: PathBuf,
}

fn wired() -> Wired {
    let dir = TempDir::new().expect("a scratch directory is available");
    let seed = init_repo(&dir.path().join("kanban.seed"));
    let bug = dir.path().join("kanban.kan-t2");
    git(
        &seed,
        &[
            "clone",
            seed.to_str().expect("utf-8"),
            bug.to_str().expect("utf-8"),
        ],
    );
    git(&bug, &["checkout", "-b", "kan-t2"]);
    fs::write(bug.join("fix.md"), "bug fix\n").expect("the bug file is written");
    git(&bug, &["add", "."]);
    git(&bug, &["commit", "-m", "bug fix"]);

    let database_path = dir.path().join("kanban.sqlite");
    let mut database = Database::open(&database_path).expect("a scratch database opens");
    database
        .migrate(&AllowAllMigrations)
        .expect("the migrations apply");
    let projects = Arc::new(SqliteProjectStore::new(&database));
    let workspaces = Arc::new(SqliteWorkspaceStore::new(&database));
    let initiatives = Arc::new(SqliteInitiativeStore::new(&database));
    let specs = Arc::new(SqliteSpecStore::new(&database));
    let plans = Arc::new(SqlitePlanStore::new(&database));
    let tickets = Arc::new(SqliteTicketStore::new(&database));
    let lanes = Arc::new(SqliteLaneStore::new(&database));
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
    core.register_plans(plans.clone(), projects.clone(), specs.clone())
        .expect("the plan operations register");
    core.register_specs(specs.clone(), projects.clone(), plans)
        .expect("the spec operations register");
    core.register_tickets(
        tickets.clone(),
        projects.clone(),
        specs.clone(),
        Arc::new(kanban_storage::SqliteEvidenceStore::new(
            &database,
            dir.path().join("attachments"),
        )),
    )
    .expect("the ticket operations register");
    core.register_lanes(
        lanes.clone(),
        projects.clone(),
        workspaces.clone(),
        tickets.clone(),
    )
    .expect("the lane operations register");
    core.register_landings(
        Arc::new(kanban_storage::SqliteLandingStore::new(&database)),
        projects.clone(),
        specs,
        tickets,
        workspaces,
        lanes,
        Arc::new(kanban_service::git_landing::LocalGitLanding),
    )
    .expect("the landing operations register");
    let registration = ProjectRegistration::new(
        "CORE",
        "Control plane",
        seed.to_str().expect("utf-8"),
        seed.to_str().expect("utf-8"),
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
    Wired {
        core,
        _dir: dir,
        seed,
        bug,
    }
}

fn attached_bug() -> (Wired, serde_json::Value, serde_json::Value) {
    let wired = wired();
    let spec = wired
        .core
        .command(
            "spec.create",
            &json!({
                "mutation": mutation(0, "spec"),
                "project_id": 1,
                "content": {
                    "name": "Landing",
                    "short_description": "Land through integration.",
                    "problem_statement": "Ad-hoc landing.",
                    "solution": "Guarded topology.",
                    "user_stories": "US7",
                    "implementation_decisions": "Lane to Spec branch.",
                    "testing_decisions": "Real git fixtures.",
                    "out_of_scope": "Recovery.",
                    "further_notes": "",
                },
            }),
        )
        .expect("the Spec is authored");
    let ticket = wired
        .core
        .command(
            "ticket.create",
            &json!({
                "mutation": mutation(0, "bug-attached"),
                "project_id": 1,
                "kind": "bug",
                "priority": "high",
                "title": "Landing drops the integration branch",
                "actual_behaviour": "The integration branch is dropped after a review lands.",
                "reporter_evidence": "The landing log names the drop immediately after the merge.",
                "spec_id": spec["id"],
            }),
        )
        .expect("the Bug is attached");
    assign_bug_lane(&wired, &ticket);
    (wired, spec, ticket)
}

#[test]
fn standalone_bug_landing_refuses_an_attached_active_spec() {
    let (wired, _, ticket) = attached_bug();
    let error = wired
        .core
        .command(
            "landing.bug",
            &json!({
                "mutation": mutation(0, "bug-wrong"),
                "project_id": 1,
                "ticket_id": ticket["id"],
                "from_path": wired.bug.to_str().expect("utf-8"),
                "into_path": wired.seed.to_str().expect("utf-8"),
            }),
        )
        .expect_err("an attached Bug cannot skip integration");
    assert_eq!(error.code, ErrorCode::InvalidRequest);
    assert!(error.message.contains("active Spec"), "{error:?}");
}

#[test]
fn standalone_bug_landing_merges_through_the_seed() {
    let wired = wired();
    let ticket = wired
        .core
        .command(
            "ticket.create",
            &json!({
                "mutation": mutation(0, "bug-ok"),
                "project_id": 1,
                "kind": "bug",
                "priority": "high",
                "title": "Landing drops the integration branch",
                "actual_behaviour": "The integration branch is dropped after a review lands.",
                "reporter_evidence": "The landing log names the drop immediately after the merge.",
            }),
        )
        .expect("the standalone Bug is created");
    assign_bug_lane(&wired, &ticket);
    let landed = wired
        .core
        .command(
            "landing.bug",
            &json!({
                "mutation": mutation(0, "bug-land"),
                "project_id": 1,
                "ticket_id": ticket["id"],
                "from_path": wired.bug.to_str().expect("utf-8"),
                "into_path": wired.seed.to_str().expect("utf-8"),
            }),
        )
        .expect("the standalone Bug lands through the Seed");
    assert_eq!(landed["kind"], "standalone_bug");
    let log = Command::new("git")
        .args([
            "-C",
            wired.seed.to_str().expect("utf-8"),
            "log",
            "--oneline",
        ])
        .output()
        .expect("the seed log reads");
    let history = String::from_utf8(log.stdout).expect("the log is UTF-8");
    assert!(history.contains("bug fix"), "{history}");
}

fn assign_bug_lane(wired: &Wired, ticket: &serde_json::Value) {
    let workspace = wired
        .core
        .command(
            "workspace.register",
            &json!({
                "mutation": mutation(0, "bug-workspace"), "project_id": 1, "path": wired.bug,
            }),
        )
        .unwrap();
    let lane = wired
        .core
        .command(
            "lane.create",
            &json!({
                "mutation": mutation(0, "bug-lane"), "project_id": 1,
            }),
        )
        .unwrap();
    let lane = wired
        .core
        .command(
            "lane.workspace.assign",
            &json!({
                "mutation": mutation(lane["version"].as_u64().unwrap(), "bug-lane-workspace"),
                "lane_id": lane["id"], "workspace_id": workspace["id"],
            }),
        )
        .unwrap();
    wired
        .core
        .command(
            "lane.ticket.assign",
            &json!({
                "mutation": mutation(lane["version"].as_u64().unwrap(), "bug-lane-ticket"),
                "lane_id": lane["id"], "ticket_id": ticket["id"],
            }),
        )
        .unwrap();
}

#[test]
fn standalone_bug_landing_allows_an_inactive_spec_without_an_integration() {
    for state in ["complete", "cancelled"] {
        let (wired, spec, ticket) = attached_bug();
        let conn = rusqlite::Connection::open(wired._dir.path().join("kanban.sqlite")).unwrap();
        conn.execute(
            "UPDATE specs SET execution = ?1 WHERE id = ?2",
            rusqlite::params![state, spec["id"].as_i64().unwrap()],
        )
        .unwrap();
        let result = wired.core.command(
            "landing.bug",
            &json!({
                "mutation": mutation(0, "inactive-bug-land"), "project_id": 1,
                "ticket_id": ticket["id"], "from_path": wired.bug, "into_path": wired.seed,
            }),
        );
        assert!(
            result.is_ok(),
            "a {state} Spec does not require integration landing: {result:?}"
        );
        assert!(wired.seed.join("fix.md").exists());
    }
}
