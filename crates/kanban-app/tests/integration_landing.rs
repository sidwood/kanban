//! App gate for Spec integration landing (KAN-T52): Ticket Lanes
//! land into the Spec integration branch, a final integration review
//! lands through the Seed, and paths outside that topology are
//! refused and recorded.

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

fn current_branch(dir: &Path) -> String {
    let output = Command::new("git")
        .args([
            "-C",
            dir.to_str().expect("the path is UTF-8"),
            "branch",
            "--show-current",
        ])
        .output()
        .expect("the current branch reads");
    String::from_utf8(output.stdout)
        .expect("the branch is UTF-8")
        .trim()
        .to_owned()
}

struct Wired {
    core: Core,
    _dir: TempDir,
    seed: PathBuf,
    integration: PathBuf,
    lane: PathBuf,
}

fn wired() -> Wired {
    let dir = TempDir::new().expect("a scratch directory is available");
    let seed = init_repo(&dir.path().join("kanban.seed"));
    git(&seed, &["checkout", "-b", "kan-s1"]);
    git(&seed, &["commit", "--allow-empty", "-m", "integration"]);
    git(&seed, &["checkout", "main"]);
    let integration = dir.path().join("kanban.kan-s1");
    git(
        &seed,
        &[
            "clone",
            seed.to_str().expect("utf-8"),
            integration.to_str().expect("utf-8"),
        ],
    );
    git(&integration, &["checkout", "kan-s1"]);
    let lane = dir.path().join("kanban.kan-t1");
    git(
        &seed,
        &[
            "clone",
            seed.to_str().expect("utf-8"),
            lane.to_str().expect("utf-8"),
        ],
    );
    git(&lane, &["checkout", "-b", "kan-t1", "origin/kan-s1"]);
    fs::write(lane.join("lane.md"), "ticket work\n").expect("the lane file is written");
    git(&lane, &["add", "."]);
    git(&lane, &["commit", "-m", "ticket work"]);

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

    core.command(
        "workspace.register",
        &json!({
            "mutation": mutation(0, "integration-workspace"), "project_id": 1,
            "path": integration.to_str().unwrap(),
        }),
    )
    .unwrap();

    Wired {
        core,
        _dir: dir,
        seed,
        integration,
        lane,
    }
}

#[test]
fn integration_landing_refuses_a_lane_that_does_not_target_the_spec_branch() {
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
    wired
        .core
        .command(
            "spec.integration.claim",
            &json!({
                "mutation": mutation(spec["version"].as_u64().unwrap(), "claim"),
                "spec_id": spec["id"],
                "branch": "kan-s1",
                "workspace_path": wired.integration.to_str().expect("utf-8"),
            }),
        )
        .expect("the Spec owns its integration branch");
    wired
        .core
        .command(
            "workspace.register",
            &json!({
                "mutation": mutation(0, "lane-ws"),
                "project_id": 1,
                "path": wired.lane.to_str().expect("utf-8"),
            }),
        )
        .expect("the lane workspace registers");
    let error = wired
        .core
        .command(
            "landing.lane",
            &json!({
                "mutation": mutation(0, "land-wrong"),
                "project_id": 1,
                "spec_id": spec["id"],
                "from_path": wired.lane.to_str().expect("utf-8"),
                "into_path": wired.seed.to_str().expect("utf-8"),
            }),
        )
        .expect_err("a lane cannot land into the Seed");
    assert_eq!(error.code, ErrorCode::InvalidRequest);
    assert_eq!(current_branch(&wired.seed), "main");
    assert_eq!(current_branch(&wired.integration), "kan-s1");
}

fn authored_spec(core: &kanban_app::Core, key: &str) -> serde_json::Value {
    core.command(
        "spec.create",
        &json!({
            "mutation": mutation(0, key),
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
    .expect("the Spec is authored")
}

fn claim_integration(core: &kanban_app::Core, spec: &serde_json::Value, path: &str, key: &str) {
    core.command(
        "spec.integration.claim",
        &json!({
            "mutation": mutation(spec["version"].as_u64().unwrap(), key),
            "spec_id": spec["id"],
            "branch": "kan-s1",
            "workspace_path": path,
        }),
    )
    .expect("the Spec owns its integration branch");
}

#[test]
fn integration_landing_merges_a_lane_into_the_spec_branch() {
    let wired = wired();
    let spec = authored_spec(&wired.core, "spec-ok");
    claim_integration(
        &wired.core,
        &spec,
        wired.integration.to_str().expect("utf-8"),
        "claim-ok",
    );
    assign_ticket_lane(&wired, &spec);
    let previous_tip = head_tip(&wired.integration);
    let landed = wired
        .core
        .command(
            "landing.lane",
            &json!({
                "mutation": mutation(0, "land-ok"),
                "project_id": 1,
                "spec_id": spec["id"],
                "from_path": wired.lane.to_str().expect("utf-8"),
                "into_path": wired.integration.to_str().expect("utf-8"),
            }),
        )
        .expect("the lane lands into the Spec branch");
    assert_eq!(landed["kind"], "lane");
    assert_eq!(landed["from_tip"], head_tip(&wired.lane));
    assert_eq!(landed["into_tip"], previous_tip);
    assert_eq!(landed["landed_tip"], head_tip(&wired.integration));
    assert_eq!(current_branch(&wired.integration), "kan-s1");
    let log = Command::new("git")
        .args([
            "-C",
            wired.integration.to_str().expect("utf-8"),
            "log",
            "--oneline",
        ])
        .output()
        .expect("the integration log reads");
    let history = String::from_utf8(log.stdout).expect("the log is UTF-8");
    assert!(history.contains("ticket work"), "{history}");
}

#[test]
fn integration_landing_requires_review_before_the_seed() {
    let wired = wired();
    let spec = authored_spec(&wired.core, "spec-seed");
    claim_integration(
        &wired.core,
        &spec,
        wired.integration.to_str().expect("utf-8"),
        "claim-seed",
    );
    let error = wired
        .core
        .command(
            "landing.seed",
            &json!({
                "mutation": mutation(0, "seed-too-soon"),
                "project_id": 1,
                "spec_id": spec["id"],
                "from_path": wired.integration.to_str().expect("utf-8"),
                "into_path": wired.seed.to_str().expect("utf-8"),
            }),
        )
        .expect_err("the Seed waits for integration review");
    assert_eq!(error.code, ErrorCode::InvalidRequest);
    wired
        .core
        .command(
            "spec.integration.approve",
            &json!({
                "mutation": mutation(spec["version"].as_u64().unwrap(), "approve"),
                "spec_id": spec["id"],
                "reviewed_tip": head_tip(&wired.integration),
                "reviewer": "operator", "evidence": "Combined-result acceptance review",
            }),
        )
        .expect("the combined result is approved");
    let landed = wired
        .core
        .command(
            "landing.seed",
            &json!({
                "mutation": mutation(0, "seed-ok"),
                "project_id": 1,
                "spec_id": spec["id"],
                "from_path": wired.integration.to_str().expect("utf-8"),
                "into_path": wired.seed.to_str().expect("utf-8"),
            }),
        )
        .expect("the approved Spec lands through the Seed");
    assert_eq!(landed["kind"], "seed");
    assert_eq!(current_branch(&wired.seed), "main");
}

#[test]
fn integration_landing_refuses_content_added_after_approval() {
    let wired = wired();
    let spec = authored_spec(&wired.core, "spec-stale");
    claim_integration(
        &wired.core,
        &spec,
        wired.integration.to_str().unwrap(),
        "claim-stale",
    );
    wired
        .core
        .command(
            "spec.integration.approve",
            &json!({
                "mutation": mutation(spec["version"].as_u64().unwrap(), "approve-stale"),
                "spec_id": spec["id"], "reviewed_tip": head_tip(&wired.integration),
                "reviewer": "operator", "evidence": "Combined-result acceptance review",
            }),
        )
        .expect("the integration is approved");
    fs::write(wired.integration.join("unreviewed.md"), "not reviewed\n").unwrap();
    git(&wired.integration, &["add", "."]);
    git(&wired.integration, &["commit", "-m", "Unreviewed change"]);
    let error = wired.core.command("landing.seed", &json!({
        "mutation": mutation(0, "stale-seed"), "project_id": 1, "spec_id": spec["id"],
        "from_path": wired.integration.to_str().unwrap(), "into_path": wired.seed.to_str().unwrap(),
    })).expect_err("new content must not ride an old integration approval");
    assert_eq!(error.code, ErrorCode::InvalidRequest);
    assert!(!wired.seed.join("unreviewed.md").exists());
}

#[test]
fn integration_landing_refuses_an_unowned_workspace_with_the_right_branch_name() {
    let wired = wired();
    let spec = authored_spec(&wired.core, "spec-impostor");
    claim_integration(
        &wired.core,
        &spec,
        wired.integration.to_str().unwrap(),
        "claim-impostor",
    );
    assign_ticket_lane(&wired, &spec);
    let impostor = wired._dir.path().join("impostor");
    git(
        &wired.seed,
        &[
            "clone",
            wired.integration.to_str().unwrap(),
            impostor.to_str().unwrap(),
        ],
    );
    let result = wired.core.command(
        "landing.lane",
        &json!({
            "mutation": mutation(0, "impostor-land"), "project_id": 1, "spec_id": spec["id"],
            "from_path": wired.lane.to_str().unwrap(), "into_path": impostor.to_str().unwrap(),
        }),
    );
    assert!(
        result.is_err(),
        "matching a branch name does not confer Workspace ownership: {result:?}"
    );
    assert!(!impostor.join("lane.md").exists());
}

fn assign_ticket_lane(wired: &Wired, spec: &serde_json::Value) {
    let core = &wired.core;
    let ticket = core
        .command(
            "ticket.create",
            &json!({
                "mutation": mutation(0, "lane-ticket"), "project_id": 1,
                "kind": "implementation", "priority": "normal", "title": "Implement landing",
                "slice": "Deliver guarded landing through integration.",
                "criteria": [{"outcome": "Land through the Spec", "stories": ["S1-US7"]}],
                "spec_id": spec["id"],
            }),
        )
        .unwrap();
    let workspace = core
        .command(
            "workspace.register",
            &json!({
                "mutation": mutation(0, "lane-workspace"), "project_id": 1,
                "path": wired.lane.to_str().unwrap(),
            }),
        )
        .unwrap();
    let lane = core
        .command(
            "lane.create",
            &json!({
                "mutation": mutation(0, "create-lane"), "project_id": 1,
            }),
        )
        .unwrap();
    let lane = core
        .command(
            "lane.workspace.assign",
            &json!({
                "mutation": mutation(lane["version"].as_u64().unwrap(), "assign-workspace"),
                "lane_id": lane["id"], "workspace_id": workspace["id"],
            }),
        )
        .unwrap();
    core.command(
        "lane.ticket.assign",
        &json!({
            "mutation": mutation(lane["version"].as_u64().unwrap(), "assign-ticket"),
            "lane_id": lane["id"], "ticket_id": ticket["id"],
        }),
    )
    .unwrap();
}

#[test]
fn integration_landing_requires_a_ticket_lane_not_an_arbitrary_clone() {
    let wired = wired();
    let spec = authored_spec(&wired.core, "spec-no-lane");
    claim_integration(
        &wired.core,
        &spec,
        wired.integration.to_str().unwrap(),
        "claim-no-lane",
    );
    let result = wired.core.command("landing.lane", &json!({
        "mutation": mutation(0, "unassigned-land"), "project_id": 1, "spec_id": spec["id"],
        "from_path": wired.lane.to_str().unwrap(), "into_path": wired.integration.to_str().unwrap(),
    }));
    assert!(
        result.is_err(),
        "an arbitrary clone is not a Ticket Lane: {result:?}"
    );
    assert!(!wired.integration.join("lane.md").exists());
}

#[test]
fn integration_landing_refuses_dirty_workspaces_before_git_effects() {
    for dirty_source in [true, false] {
        let wired = wired();
        let spec = authored_spec(&wired.core, "spec-dirty");
        claim_integration(
            &wired.core,
            &spec,
            wired.integration.to_str().unwrap(),
            "claim-dirty",
        );
        assign_ticket_lane(&wired, &spec);
        let dirty = if dirty_source {
            &wired.lane
        } else {
            &wired.integration
        };
        fs::write(dirty.join("local-notes.md"), "Keep this local work\n").unwrap();
        let result = wired.core.command("landing.lane", &json!({
            "mutation": mutation(0, "dirty-land"), "project_id": 1, "spec_id": spec["id"],
            "from_path": wired.lane.to_str().unwrap(), "into_path": wired.integration.to_str().unwrap(),
        }));
        assert!(
            result.is_err(),
            "dirty source={dirty_source} must refuse: {result:?}"
        );
        assert_eq!(
            fs::read_to_string(dirty.join("local-notes.md")).unwrap(),
            "Keep this local work\n"
        );
        assert!(!wired.integration.join("lane.md").exists());
    }
}

#[test]
fn integration_landing_preserves_intent_when_outcome_commit_fails() {
    let wired = wired();
    let spec = authored_spec(&wired.core, "spec-commit-failure");
    claim_integration(
        &wired.core,
        &spec,
        wired.integration.to_str().unwrap(),
        "claim-commit-failure",
    );
    assign_ticket_lane(&wired, &spec);
    let conn = rusqlite::Connection::open(wired._dir.path().join("kanban.sqlite")).unwrap();
    conn.execute_batch(
        "CREATE TRIGGER fail_landing_outcome BEFORE INSERT ON idempotency_outcomes
        WHEN NEW.idempotency_key = 'failed-land' BEGIN SELECT RAISE(ABORT, 'outcome failed'); END;",
    )
    .unwrap();
    let mut request = json!({
        "mutation": mutation(0, "failed-land"), "project_id": 1, "spec_id": spec["id"],
        "from_path": wired.lane.to_str().unwrap(), "into_path": wired.integration.to_str().unwrap(),
    });
    assert!(wired.core.command("landing.lane", &request).is_err());
    assert!(
        wired.integration.join("lane.md").exists(),
        "the external merge happened"
    );
    let started: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM timeline_events
        WHERE json_extract(detail, '$.action') = 'landing_started'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        started, 1,
        "a durable intent must survive the failed outcome transaction"
    );
    conn.execute_batch("DROP TRIGGER fail_landing_outcome")
        .unwrap();
    for key in ["failed-land", "new-key"] {
        request["mutation"] = mutation(0, key);
        let error = wired
            .core
            .command("landing.lane", &request)
            .expect_err("recovery must be explicit, not another merge");
        assert!(error.message.contains("recovery"), "{}", error.message);
    }
    request["mutation"] = mutation(0, "alias-retry");
    request["from_path"] = json!(wired.lane.join("."));
    request["into_path"] = json!(wired.integration.join("."));
    let error = wired
        .core
        .command("landing.lane", &request)
        .expect_err("path aliases cannot bypass a pending external effect");
    assert!(error.message.contains("recovery"), "{error:?}");
}

#[test]
fn integration_landing_approval_requires_the_explicitly_reviewed_tip() {
    let wired = wired();
    let spec = authored_spec(&wired.core, "review-spec");
    claim_integration(
        &wired.core,
        &spec,
        wired.integration.to_str().unwrap(),
        "review-claim",
    );
    let mut request = json!({
        "mutation": mutation(1, "explicit-review"), "spec_id": spec["id"],
        "reviewed_tip": "0".repeat(40), "reviewer": "operator",
        "evidence": "Combined-result acceptance review"
    });
    let error = wired
        .core
        .command("spec.integration.approve", &request)
        .expect_err("a review of another tip cannot approve the current integration");
    assert!(error.message.contains("reviewed tip"), "{error:?}");
    request["reviewed_tip"] = json!(head_tip(&wired.integration));
    wired
        .core
        .command("spec.integration.approve", &request)
        .expect("the explicitly reviewed tip is accepted");
}

#[test]
fn integration_landing_refuses_a_ticket_lane_branched_from_the_seed() {
    let wired = wired();
    let spec = authored_spec(&wired.core, "wrong-base-spec");
    claim_integration(
        &wired.core,
        &spec,
        wired.integration.to_str().unwrap(),
        "wrong-base-claim",
    );
    assign_ticket_lane(&wired, &spec);
    git(&wired.lane, &["reset", "--hard", "origin/main"]);
    fs::write(
        wired.lane.join("wrong-base.md"),
        "Not based on the integration branch",
    )
    .unwrap();
    git(&wired.lane, &["add", "."]);
    git(&wired.lane, &["commit", "-m", "Work on wrong base"]);
    let error = wired
        .core
        .command(
            "landing.lane",
            &json!({
                "mutation": mutation(0, "wrong-base-land"), "project_id": 1,
                "spec_id": spec["id"], "from_path": wired.lane, "into_path": wired.integration,
            }),
        )
        .expect_err("Ticket Lanes must branch from the Spec integration");
    assert!(error.message.contains("integration base"), "{error:?}");
    assert!(!wired.integration.join("wrong-base.md").exists());
}

#[test]
fn integration_landing_claim_requires_an_owned_workspace_on_its_branch() {
    for scenario in [
        "seed",
        "unregistered",
        "wrong-branch",
        "already-claimed",
        "shared-workspace",
    ] {
        let wired = wired();
        let mut spec = authored_spec(&wired.core, "claim-guard-spec");
        if matches!(scenario, "already-claimed" | "shared-workspace") {
            claim_integration(
                &wired.core,
                &spec,
                wired.integration.to_str().unwrap(),
                "original-claim",
            );
        }
        if scenario == "shared-workspace" {
            spec = authored_spec(&wired.core, "competing-spec");
        }
        let (path, branch) = match scenario {
            "seed" => (&wired.seed, "main"),
            "unregistered" => (&wired.lane, "kan-t1"),
            "wrong-branch" => (&wired.integration, "invented-branch"),
            _ => (&wired.integration, "kan-s1"),
        };
        let result = wired.core.command(
            "spec.integration.claim",
            &json!({
                "mutation": mutation(1, "invalid-claim"), "spec_id": spec["id"],
                "branch": branch, "workspace_path": path,
            }),
        );
        assert!(
            result.is_err(),
            "{scenario} must not create or overwrite a Spec claim: {result:?}"
        );
    }
}

#[test]
fn integration_landing_rechecks_seed_branch_and_workspace_liveness() {
    for scenario in ["wrong-branch", "detached", "retired", "archived"] {
        let wired = wired();
        let spec = authored_spec(&wired.core, "live-spec");
        claim_integration(
            &wired.core,
            &spec,
            wired.integration.to_str().unwrap(),
            "live-claim",
        );
        wired.core.command("spec.integration.approve", &json!({
            "mutation": mutation(1, "live-review"), "spec_id": spec["id"],
            "reviewed_tip": head_tip(&wired.integration), "reviewer": "operator", "evidence": "Combined-result review",
        })).unwrap();
        let before = head_tip(&wired.seed);
        match scenario {
            "wrong-branch" => git(&wired.seed, &["checkout", "-b", "wrong-target"]),
            "detached" => git(&wired.seed, &["checkout", "--detach"]),
            "retired" => {
                let conn =
                    rusqlite::Connection::open(wired._dir.path().join("kanban.sqlite")).unwrap();
                conn.execute(
                    "UPDATE workspaces SET retired = 1 WHERE path = ?1",
                    [wired.integration.to_str().unwrap()],
                )
                .unwrap();
            }
            _ => {
                let conn =
                    rusqlite::Connection::open(wired._dir.path().join("kanban.sqlite")).unwrap();
                conn.execute("UPDATE projects SET archived = 1 WHERE id = 1", [])
                    .unwrap();
            }
        }
        let result = wired.core.command(
            "landing.seed",
            &json!({
                "mutation": mutation(0, "live-land"), "project_id": 1, "spec_id": spec["id"],
                "from_path": wired.integration, "into_path": wired.seed,
            }),
        );
        assert!(
            result.is_err(),
            "{scenario} must refuse before Git: {result:?}"
        );
        assert_eq!(head_tip(&wired.seed), before);
    }
}

fn head_tip(dir: &Path) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["rev-parse", "HEAD"])
        .output()
        .unwrap();
    assert!(output.status.success());
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}
