//! Landing recovery after Git succeeds and the durable outcome write
//! fails (KAN-T146): an explicit audited reconcile completes or
//! releases the reservation so later landings are not permanently
//! refused.

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

fn mutation(version: u64, key: impl AsRef<str>) -> serde_json::Value {
    json!({
        "optimistic_version": version,
        "idempotency_key": key.as_ref(),
    })
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

fn merge_in_progress(dir: &Path) -> bool {
    Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["rev-parse", "-q", "--verify", "MERGE_HEAD"])
        .status()
        .expect("git rev-parse MERGE_HEAD runs")
        .success()
}

fn open_db(wired: &Wired) -> rusqlite::Connection {
    rusqlite::Connection::open(wired._dir.path().join("kanban.sqlite")).unwrap()
}

fn fail_outcome(wired: &Wired, key: &str) {
    let conn = open_db(wired);
    conn.execute_batch(&format!(
        "CREATE TRIGGER fail_landing_outcome BEFORE INSERT ON idempotency_outcomes
        WHEN NEW.idempotency_key = '{key}' BEGIN SELECT RAISE(ABORT, 'outcome failed'); END;"
    ))
    .unwrap();
}

fn drop_outcome_failure(wired: &Wired) {
    open_db(wired)
        .execute_batch("DROP TRIGGER IF EXISTS fail_landing_outcome")
        .unwrap();
}

fn count_sql(wired: &Wired, sql: &str) -> i64 {
    open_db(wired).query_row(sql, [], |row| row.get(0)).unwrap()
}

fn incomplete_intents(wired: &Wired) -> i64 {
    count_sql(
        wired,
        "SELECT COUNT(*) FROM landing_intents WHERE completed = 0",
    )
}

fn landing_rows(wired: &Wired) -> i64 {
    count_sql(wired, "SELECT COUNT(*) FROM landings")
}

fn ruling_summaries(wired: &Wired) -> Vec<String> {
    let conn = open_db(wired);
    let mut statement = conn
        .prepare("SELECT summary FROM rulings ORDER BY id")
        .unwrap();
    statement
        .query_map([], |row| row.get(0))
        .unwrap()
        .map(|row| row.unwrap())
        .collect()
}

fn timeline_actions(wired: &Wired) -> Vec<String> {
    let conn = open_db(wired);
    let mut statement = conn
        .prepare(
            "SELECT json_extract(detail, '$.action') FROM timeline_events
             WHERE json_extract(detail, '$.action') IS NOT NULL ORDER BY id",
        )
        .unwrap();
    statement
        .query_map([], |row| row.get(0))
        .unwrap()
        .map(|row| row.unwrap())
        .collect()
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
    let rulings = Arc::new(kanban_storage::SqliteRulingStore::new(&database));
    core.register_rulings(rulings.clone(), projects.clone())
        .expect("the ruling operations register");
    core.register_landings(
        Arc::new(kanban_storage::SqliteLandingStore::new(&database)),
        projects.clone(),
        specs,
        tickets,
        workspaces,
        lanes,
        Arc::new(kanban_storage::SqliteReviewExecutionStore::new(&database)),
        Arc::new(kanban_storage::SqliteCriterionBindingStore::new(&database)),
        Arc::new(kanban_service::git_landing::LocalGitLanding),
    )
    .expect("the landing operations register");
    common::landing_review::register_source_review(&mut core, &database, dir.path());
    common::landing_review::seed_review_profiles(&core);

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
                "user_stories": "- CORE-S1-US7: As an operator, I want work to land through integration.",
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

fn assign_ticket_lane(wired: &Wired, spec: &serde_json::Value, prefix: &str) -> serde_json::Value {
    let core = &wired.core;
    let ticket = core
        .command(
            "ticket.create",
            &json!({
                "mutation": mutation(0, format!("{prefix}-ticket")), "project_id": 1,
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
                "mutation": mutation(0, format!("{prefix}-workspace")), "project_id": 1,
                "path": wired.lane.to_str().unwrap(),
            }),
        )
        .unwrap();
    let lane = core
        .command(
            "lane.create",
            &json!({
                "mutation": mutation(0, format!("{prefix}-create-lane")), "project_id": 1,
            }),
        )
        .unwrap();
    let lane = core
        .command(
            "lane.workspace.assign",
            &json!({
                "mutation": mutation(lane["version"].as_u64().unwrap(), format!("{prefix}-assign-workspace")),
                "lane_id": lane["id"], "workspace_id": workspace["id"],
            }),
        )
        .unwrap();
    core.command(
        "lane.ticket.assign",
        &json!({
            "mutation": mutation(lane["version"].as_u64().unwrap(), format!("{prefix}-assign-ticket")),
            "lane_id": lane["id"], "ticket_id": ticket["id"],
        }),
    )
    .unwrap();
    ticket
}

fn claimed_spec(wired: &Wired, prefix: &str) -> serde_json::Value {
    let spec = authored_spec(&wired.core, &format!("{prefix}-spec"));
    claim_integration(
        &wired.core,
        &spec,
        wired.integration.to_str().expect("utf-8"),
        &format!("{prefix}-claim"),
    );
    spec
}

fn reviewed_lane(wired: &Wired, spec: &serde_json::Value, prefix: &str) {
    let ticket = assign_ticket_lane(wired, spec, prefix);
    common::landing_review::complete_source_review(
        &wired.core,
        &ticket,
        Some(spec),
        &wired.lane,
        "lane.md",
        &format!("{prefix}-review"),
        true,
    );
}

fn spec_version(core: &kanban_app::Core, spec: &serde_json::Value) -> u64 {
    core.query("spec.get", &json!({ "spec_id": spec["id"] }))
        .expect("the Spec reads")["spec"]["version"]
        .as_u64()
        .expect("the Spec version is a number")
}

fn land_lane_request(wired: &Wired, spec: &serde_json::Value, key: &str) -> serde_json::Value {
    json!({
        "mutation": mutation(0, key),
        "project_id": 1,
        "spec_id": spec["id"],
        "from_path": wired.lane.to_str().unwrap(),
        "into_path": wired.integration.to_str().unwrap(),
    })
}

fn land_seed_request(wired: &Wired, spec: &serde_json::Value, key: &str) -> serde_json::Value {
    json!({
        "mutation": mutation(0, key),
        "project_id": 1,
        "spec_id": spec["id"],
        "from_path": wired.integration.to_str().unwrap(),
        "into_path": wired.seed.to_str().unwrap(),
    })
}

fn refuse_later_landings(wired: &Wired, spec: &serde_json::Value, original_key: &str) {
    for key in [original_key, "fresh-key"] {
        let error = wired
            .core
            .command("landing.lane", &land_lane_request(wired, spec, key))
            .expect_err("recovery must be explicit, not another merge");
        assert!(
            error.message.contains("recovery"),
            "later landing must stay reserved until reconcile: {error:?}"
        );
    }
}

fn replay_completed_landing(
    wired: &Wired,
    command: &str,
    request: serde_json::Value,
    kind: &str,
    landed_tip: &serde_json::Value,
) {
    let replayed = wired
        .core
        .command(command, &request)
        .expect("the original key replays the completed landing");
    assert_eq!(replayed["kind"], kind);
    assert_eq!(replayed["landed_tip"], *landed_tip);
}

fn assert_key_is_not_reserved(wired: &Wired, command: &str, request: serde_json::Value) {
    if let Err(error) = wired.core.command(command, &request) {
        assert!(
            !error
                .message
                .contains("a prior landing requires explicit recovery"),
            "recovery must not leave a landing key refusing: {error:?}"
        );
    }
}

fn reconcile(
    wired: &Wired,
    intent_key: &str,
    policy: &str,
    key: &str,
) -> Result<serde_json::Value, kanban_dto::ApiError> {
    wired.core.command(
        "landing.reconcile",
        &json!({
            "mutation": mutation(0, key),
            "project_id": 1,
            "intent_key": intent_key,
            "policy": policy,
        }),
    )
}

#[test]
fn landing_recovery_completes_git_succeeded_before_durable_completion() {
    let wired = wired();
    let spec = claimed_spec(&wired, "complete");
    reviewed_lane(&wired, &spec, "complete");
    fail_outcome(&wired, "recovery-land");
    assert!(
        wired
            .core
            .command(
                "landing.lane",
                &land_lane_request(&wired, &spec, "recovery-land")
            )
            .is_err()
    );
    assert!(
        wired.integration.join("lane.md").exists(),
        "the external merge happened"
    );
    assert_eq!(incomplete_intents(&wired), 1);
    assert_eq!(landing_rows(&wired), 0);
    drop_outcome_failure(&wired);
    refuse_later_landings(&wired, &spec, "recovery-land");

    let recovered = reconcile(&wired, "recovery-land", "complete", "reconcile-complete")
        .expect("Git success is completed through the product operation");
    assert_eq!(recovered["policy"], "complete");
    assert_eq!(
        recovered["ruling_summary"],
        "git_succeeded_before_durable_completion"
    );
    assert!(recovered["ruling_id"].as_u64().unwrap() >= 1);
    assert_eq!(recovered["landing"]["kind"], "lane");
    assert_eq!(
        recovered["landing"]["landed_tip"],
        head_tip(&wired.integration)
    );
    assert_eq!(recovered["observed_tip"], head_tip(&wired.integration));
    assert_eq!(incomplete_intents(&wired), 0);
    assert_eq!(landing_rows(&wired), 1);
    assert!(
        ruling_summaries(&wired)
            .iter()
            .any(|summary| summary == "git_succeeded_before_durable_completion"),
        "the named policy is recorded as a ruling: {:?}",
        ruling_summaries(&wired)
    );
    assert!(
        timeline_actions(&wired)
            .iter()
            .any(|action| action == "landing_reconciled"),
        "reconcile is on the timeline: {:?}",
        timeline_actions(&wired)
    );

    replay_completed_landing(
        &wired,
        "landing.lane",
        land_lane_request(&wired, &spec, "recovery-land"),
        "lane",
        &recovered["landing"]["landed_tip"],
    );
    assert_eq!(landing_rows(&wired), 1);
    assert_key_is_not_reserved(
        &wired,
        "landing.lane",
        land_lane_request(&wired, &spec, "later-lane"),
    );
}

#[test]
fn landing_recovery_releases_a_conflicted_merge_without_database_edits() {
    let wired = wired();
    let spec = claimed_spec(&wired, "conflict");
    fs::write(wired.integration.join("README.md"), "integration edit\n").unwrap();
    git(&wired.integration, &["add", "."]);
    git(&wired.integration, &["commit", "-m", "integration edit"]);
    fs::write(wired.lane.join("README.md"), "lane edit\n").unwrap();
    git(&wired.lane, &["add", "."]);
    git(&wired.lane, &["commit", "-m", "lane edit"]);
    reviewed_lane(&wired, &spec, "conflict");
    let error = wired
        .core
        .command(
            "landing.lane",
            &land_lane_request(&wired, &spec, "conflict-land"),
        )
        .expect_err("the conflicting merge is refused");
    assert_eq!(error.code, ErrorCode::Internal);
    assert!(merge_in_progress(&wired.integration));
    assert_eq!(incomplete_intents(&wired), 1);
    assert_eq!(landing_rows(&wired), 0);
    git(&wired.integration, &["merge", "--abort"]);
    assert!(
        !merge_in_progress(&wired.integration),
        "a fixture abort cleans Git but must not clear the reservation"
    );
    refuse_later_landings(&wired, &spec, "conflict-land");

    let recovered = reconcile(&wired, "conflict-land", "release", "reconcile-release")
        .expect("a conflicted merge is released through the same operation");
    assert_eq!(recovered["policy"], "release");
    assert_eq!(recovered["ruling_summary"], "landing_reservation_release");
    assert!(recovered["landing"].is_null());
    assert!(!merge_in_progress(&wired.integration));
    assert_eq!(incomplete_intents(&wired), 0);
    assert_eq!(landing_rows(&wired), 0);
    assert!(
        ruling_summaries(&wired)
            .iter()
            .any(|summary| summary == "landing_reservation_release")
    );

    for key in ["conflict-land", "after-release"] {
        assert_key_is_not_reserved(
            &wired,
            "landing.lane",
            land_lane_request(&wired, &spec, key),
        );
    }
}

#[test]
fn landing_recovery_completes_a_failed_seed_outcome() {
    let wired = wired();
    let spec = claimed_spec(&wired, "seed");
    reviewed_lane(&wired, &spec, "seed");
    wired
        .core
        .command(
            "landing.lane",
            &land_lane_request(&wired, &spec, "seed-lane"),
        )
        .expect("the lane lands first");
    wired
        .core
        .command(
            "spec.integration.approve",
            &json!({
                "mutation": mutation(spec_version(&wired.core, &spec), "seed-approve"),
                "spec_id": spec["id"],
                "reviewed_tip": head_tip(&wired.integration),
                "reviewer": "operator",
                "evidence": "Combined-result acceptance review",
            }),
        )
        .expect("the combined result is approved");
    fail_outcome(&wired, "recovery-seed");
    assert!(
        wired
            .core
            .command(
                "landing.seed",
                &land_seed_request(&wired, &spec, "recovery-seed"),
            )
            .is_err()
    );
    assert!(wired.seed.join("lane.md").exists());
    drop_outcome_failure(&wired);
    let recovered = reconcile(&wired, "recovery-seed", "complete", "reconcile-seed")
        .expect("Seed Git success is completed through the product operation");
    assert_eq!(recovered["policy"], "complete");
    assert_eq!(recovered["landing"]["kind"], "seed");
    assert_eq!(incomplete_intents(&wired), 0);
    assert_eq!(landing_rows(&wired), 2);
    replay_completed_landing(
        &wired,
        "landing.seed",
        land_seed_request(&wired, &spec, "recovery-seed"),
        "seed",
        &recovered["landing"]["landed_tip"],
    );
    assert_eq!(landing_rows(&wired), 2);
    assert_key_is_not_reserved(
        &wired,
        "landing.seed",
        land_seed_request(&wired, &spec, "later-seed"),
    );
}

#[test]
fn landing_recovery_refuses_a_policy_that_does_not_match_git() {
    let wired = wired();
    let spec = claimed_spec(&wired, "mismatch");
    reviewed_lane(&wired, &spec, "mismatch");
    fail_outcome(&wired, "mismatch-land");
    assert!(
        wired
            .core
            .command(
                "landing.lane",
                &land_lane_request(&wired, &spec, "mismatch-land")
            )
            .is_err()
    );
    drop_outcome_failure(&wired);
    let error = reconcile(&wired, "mismatch-land", "release", "wrong-policy")
        .expect_err("a successful merge cannot be released");
    assert_eq!(error.code, ErrorCode::InvalidRequest);
    assert_eq!(incomplete_intents(&wired), 1);
    assert!(wired.integration.join("lane.md").exists());
}

#[test]
fn landing_recovery_releases_a_conflicted_seed_merge() {
    let wired = wired();
    let spec = claimed_spec(&wired, "seed-release");
    reviewed_lane(&wired, &spec, "seed-release");
    wired
        .core
        .command(
            "landing.lane",
            &land_lane_request(&wired, &spec, "seed-release-lane"),
        )
        .expect("the lane lands first");
    fs::write(wired.seed.join("README.md"), "seed edit\n").unwrap();
    git(&wired.seed, &["add", "."]);
    git(&wired.seed, &["commit", "-m", "seed edit"]);
    fs::write(wired.integration.join("README.md"), "integration edit\n").unwrap();
    git(&wired.integration, &["add", "."]);
    git(&wired.integration, &["commit", "-m", "integration edit"]);
    wired
        .core
        .command(
            "spec.integration.approve",
            &json!({
                "mutation": mutation(spec_version(&wired.core, &spec), "seed-release-approve"),
                "spec_id": spec["id"],
                "reviewed_tip": head_tip(&wired.integration),
                "reviewer": "operator",
                "evidence": "Combined-result acceptance review",
            }),
        )
        .expect("the combined result is approved");
    let error = wired
        .core
        .command(
            "landing.seed",
            &land_seed_request(&wired, &spec, "seed-release-land"),
        )
        .expect_err("the conflicting Seed merge is refused");
    assert_eq!(error.code, ErrorCode::Internal);
    assert!(merge_in_progress(&wired.seed));
    git(&wired.seed, &["merge", "--abort"]);
    let recovered = reconcile(
        &wired,
        "seed-release-land",
        "release",
        "reconcile-seed-release",
    )
    .expect("a conflicted Seed merge is released through the same operation");
    assert_eq!(recovered["policy"], "release");
    assert!(recovered["landing"].is_null());
    assert_eq!(incomplete_intents(&wired), 0);
    for key in ["seed-release-land", "later-seed"] {
        assert_key_is_not_reserved(
            &wired,
            "landing.seed",
            land_seed_request(&wired, &spec, key),
        );
    }
}
