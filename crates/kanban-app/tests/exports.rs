//! App gate for exports through the shipped local-filesystem adapter
//! and real SQLite persistence (KAN-T35): atomic writes into a
//! directory within the Seed, byte stability across renders, and the
//! proof that an export never commits, pushes, or stages anything.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use kanban_app::ProjectStore;
use kanban_app::catalog::exposed_operations;
use kanban_app::dispatch::Core;
use kanban_app::herdr::NoopHerdrProjectObserver;
use kanban_domain::ProjectRegistration;
use kanban_dto::{TimelineEntityKind, TimelineEntityRef, TimelineEventKind};
use kanban_service::LocalRepositories;
use kanban_service::export_files::LocalExportFiles;
use kanban_storage::{
    AllowAllMigrations, Database, RetentionPolicy, SqliteEvidenceStore, SqliteHerdrSettingsStore,
    SqliteIdempotencyStore, SqliteInitiativeStore, SqlitePlanStore, SqliteProjectStore,
    SqliteSpecStore, SqliteTicketStore,
};
use serde_json::{Value, json};
use tempfile::TempDir;

fn git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .args(args)
        .current_dir(dir)
        .status()
        .expect("git runs");
    assert!(status.success(), "git {:?} in {}", args, dir.display());
}

fn git_output(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .expect("git runs");
    assert!(
        output.status.success(),
        "git {:?} in {} failed",
        args,
        dir.display()
    );
    String::from_utf8(output.stdout).expect("git output is UTF-8")
}

struct Wired {
    core: Core,
    projects: Arc<SqliteProjectStore>,
}

fn wired(scratch: &Path) -> Wired {
    let database_path = scratch.join("kanban.sqlite");
    let mut database = Database::open(&database_path).expect("a scratch database opens");
    database
        .migrate(&AllowAllMigrations)
        .expect("the migrations apply");
    let projects = Arc::new(SqliteProjectStore::new(&database));
    let initiatives = Arc::new(SqliteInitiativeStore::new(&database));
    let plans = Arc::new(SqlitePlanStore::new(&database));
    let specs = Arc::new(SqliteSpecStore::new(&database));
    let tickets = Arc::new(SqliteTicketStore::new(&database));
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
    core.register_plans(plans.clone(), projects.clone(), specs.clone())
        .expect("the plan operations register");
    core.register_specs(specs.clone(), projects.clone(), plans.clone())
        .expect("the spec operations register");
    core.register_tickets(
        tickets.clone(),
        projects.clone(),
        specs.clone(),
        Arc::new(SqliteEvidenceStore::new(
            &database,
            scratch.join("attachments"),
        )),
    )
    .expect("the ticket operations register");
    core.register_exports(
        plans,
        specs,
        tickets,
        projects.clone(),
        Arc::new(LocalExportFiles),
    )
    .expect("the export operations register");
    Wired { core, projects }
}

/// A git repository standing in for the Seed Workspace.
fn seed_repository(parent: &Path) -> PathBuf {
    let seed = parent.join("kanban.seed");
    fs::create_dir_all(&seed).expect("the seed directory is created");
    git(&seed, &["init"]);
    git(&seed, &["config", "user.email", "test@example.com"]);
    git(&seed, &["config", "user.name", "Test"]);
    fs::write(seed.join("README.md"), "seed\n").expect("the seed file is written");
    git(&seed, &["add", "."]);
    git(&seed, &["commit", "-m", "initial"]);
    seed
}

fn create_project(projects: &SqliteProjectStore, seed_workspace: &str) {
    let registration = ProjectRegistration::new(
        "CORE",
        "Control plane",
        seed_workspace,
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

/// Author the fixture planning state through the served core: one
/// approved Spec inside one frozen Plan and one attached
/// Implementation Ticket.
fn author_planning_state(core: &Core) {
    let spec = core
        .command(
            "spec.create",
            &json!({
                "mutation": { "optimistic_version": 0, "idempotency_key": "spec-1" },
                "project_id": 1,
                "content": {
                    "name": "Lanes, workspaces, and Git",
                    "short_description": "Registered, observed Workspaces.",
                    "problem_statement": "Agents work in real working copies.",
                    "solution": "Guarded clone commands and deterministic exports.",
                    "user_stories": "KAN-S6-US5",
                    "implementation_decisions": "Exports write atomically.",
                    "testing_decisions": "Export tests prove byte stability.",
                    "out_of_scope": "Automatic Workspace cleanup.",
                    "further_notes": "None",
                },
            }),
        )
        .expect("the Spec authors");
    let spec_id = spec["id"].as_u64().expect("the identity is a number");

    let plan = core
        .command(
            "plan.create",
            &json!({
                "mutation": { "optimistic_version": 0, "idempotency_key": "plan-1" },
                "project_id": 1,
            }),
        )
        .expect("the Plan creates");
    let plan_id = plan["id"].as_u64().expect("the identity is a number");
    let mut version = plan["version"].as_u64().expect("the version is a number");
    let response = core
        .command(
            "plan.spec.add",
            &json!({
                "mutation": { "optimistic_version": version, "idempotency_key": "plan-add-1" },
                "plan_id": plan_id,
                "spec_number": 1,
            }),
        )
        .expect("the Spec joins the Plan");
    version = response["version"]
        .as_u64()
        .expect("the version is a number");
    core.command(
        "plan.activate",
        &json!({
            "mutation": { "optimistic_version": version, "idempotency_key": "plan-activate" },
            "plan_id": plan_id,
        }),
    )
    .expect("the Plan freezes");
    let joined = core
        .command(
            "spec.plan.join",
            &json!({
                "mutation": {
                    "optimistic_version": spec["version"].as_u64().expect("the version is a number"),
                    "idempotency_key": "spec-join",
                },
                "spec_id": spec_id,
                "plan_id": plan_id,
            }),
        )
        .expect("the Spec joins its Plan");
    core.command(
        "spec.version.approve",
        &json!({
            "mutation": {
                "optimistic_version": joined["version"].as_u64().expect("the version is a number"),
                "idempotency_key": "spec-approve",
            },
            "spec_id": spec_id,
        }),
    )
    .expect("the content approves");

    core.command(
        "ticket.create",
        &json!({
            "mutation": { "optimistic_version": 0, "idempotency_key": "ticket-1" },
            "project_id": 1,
            "kind": "implementation",
            "priority": "normal",
            "spec_id": spec_id,
            "slice": "Exports render planning state end to end",
            "criteria": [
                {
                    "outcome": "Identical state renders identical bytes.",
                    "stories": ["CORE-S1-US5"],
                },
            ],
        }),
    )
    .expect("the Implementation Ticket creates");
}

fn render(key: &str) -> Value {
    json!({
        "mutation": { "optimistic_version": 0, "idempotency_key": key },
        "project_id": 1,
        "directory": "docs/planning",
    })
}

/// Every regular file under `root`, recursively, sorted.
fn files_under(root: &Path) -> Vec<PathBuf> {
    fn walk(dir: &Path, found: &mut Vec<PathBuf>) {
        let entries = fs::read_dir(dir).expect("the directory lists");
        for entry in entries.filter_map(|entry| entry.ok()) {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, found);
            } else {
                found.push(path);
            }
        }
    }
    let mut found = Vec::new();
    walk(root, &mut found);
    found.sort();
    found
}

#[test]
fn exports_render_writes_atomic_markdown_into_the_seed() {
    let scratch = TempDir::new().expect("a scratch directory is available");
    let seed = seed_repository(scratch.path());
    let Wired { core, projects } = wired(scratch.path());
    create_project(&projects, seed.to_str().expect("the path is UTF-8"));
    author_planning_state(&core);

    let response = core
        .command("export.render", &render("render-1"))
        .expect("the render applies");

    let root = seed.join("docs").join("planning");
    let written = files_under(&root);
    let relative: Vec<String> = written
        .iter()
        .map(|path| {
            path.strip_prefix(&root)
                .expect("the path sits under the root")
                .to_string_lossy()
                .replace('\\', "/")
        })
        .collect();
    assert_eq!(
        relative,
        vec!["plans/CORE-P1.md", "specs/CORE-S1.md", "tickets/CORE-T1.md",],
        "every artifact lands inside the configured Seed directory"
    );
    assert_eq!(
        response["files"],
        json!(["plans/CORE-P1.md", "specs/CORE-S1.md", "tickets/CORE-T1.md"])
    );
    for path in &written {
        let bytes = fs::read(path).expect("the document is readable");
        assert!(
            !bytes.is_empty(),
            "the document at {} carries its rendering",
            path.display()
        );
        assert!(
            !path
                .file_name()
                .expect("the path names a file")
                .to_string_lossy()
                .contains("export-tmp"),
            "no temporary sibling survives the atomic replace"
        );
    }

    // Byte stability end to end: a second render from identical state
    // rewrites the same bytes and leaves no residue.
    let before: Vec<Vec<u8>> = written
        .iter()
        .map(|p| fs::read(p).expect("readable"))
        .collect();
    core.command("export.render", &render("render-2"))
        .expect("the second render applies");
    let after: Vec<Vec<u8>> = written
        .iter()
        .map(|p| fs::read(p).expect("readable"))
        .collect();
    assert_eq!(before, after, "identical state renders identical files");
    assert_eq!(
        files_under(&root).len(),
        written.len(),
        "no residue appears"
    );

    // A rerender over a hand edit replaces the file wholesale.
    fs::write(root.join("plans").join("CORE-P1.md"), "# hand edit\n").expect("the hand edit lands");
    core.command("export.render", &render("render-3"))
        .expect("the third render applies");
    let restored = fs::read(root.join("plans").join("CORE-P1.md")).expect("the document restores");
    assert_eq!(
        restored, before[0],
        "the atomic replace restores the rendered bytes"
    );
}

#[test]
fn exports_never_commit_push_or_stage_anything() {
    let scratch = TempDir::new().expect("a scratch directory is available");
    let seed = seed_repository(scratch.path());
    let Wired { core, projects } = wired(scratch.path());
    create_project(&projects, seed.to_str().expect("the path is UTF-8"));
    author_planning_state(&core);

    let head_before = git_output(&seed, &["rev-parse", "HEAD"]);
    let staged_before = git_output(&seed, &["diff", "--cached", "--name-only"]);
    assert_eq!(staged_before, "", "the fixture starts with nothing staged");

    core.command("export.render", &render("render-1"))
        .expect("the render applies");
    core.query(
        "export.drift",
        &json!({ "project_id": 1, "directory": "docs/planning" }),
    )
    .expect("the drift query serves");

    let head_after = git_output(&seed, &["rev-parse", "HEAD"]);
    assert_eq!(
        head_before, head_after,
        "an export must never move HEAD: nothing is committed"
    );
    let staged_after = git_output(&seed, &["diff", "--cached", "--name-only"]);
    assert_eq!(
        staged_after, "",
        "an export must never stage anything: the index stays untouched"
    );
    let status = git_output(&seed, &["status", "--porcelain"]);
    let lines: Vec<&str> = status.lines().collect();
    assert_eq!(
        lines,
        vec!["?? docs/"],
        "the export files appear as plain untracked work, never staged: {status}"
    );
    let remotes = git_output(&seed, &["remote"]);
    assert_eq!(
        remotes, "",
        "the export surface configures no remote to push through"
    );
}
