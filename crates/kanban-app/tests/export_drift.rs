//! App gate for export drift reporting through the shipped
//! local-filesystem adapter and real SQLite persistence (KAN-T35):
//! drift between the exported Markdown on disk and the current
//! planning state is reported on demand.

use std::fs;
use std::path::Path;
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
    AllowAllMigrations, Database, RetentionPolicy, SqliteHerdrSettingsStore,
    SqliteIdempotencyStore, SqliteInitiativeStore, SqlitePlanStore, SqliteProjectStore,
    SqliteSpecStore, SqliteTicketStore,
};
use serde_json::{Value, json};
use tempfile::TempDir;

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
    core.register_tickets(tickets.clone(), projects.clone(), specs.clone())
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

fn author_planning_state(core: &Core) {
    core.command(
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

    core.command(
        "ticket.create",
        &json!({
            "mutation": { "optimistic_version": 0, "idempotency_key": "ticket-1" },
            "project_id": 1,
            "kind": "task",
            "priority": "normal",
            "title": "Rotate the fleet board",
        }),
    )
    .expect("the Task Ticket creates");
}

fn render(key: &str) -> Value {
    json!({
        "mutation": { "optimistic_version": 0, "idempotency_key": key },
        "project_id": 1,
        "directory": "docs/planning",
    })
}

fn drift() -> Value {
    json!({ "project_id": 1, "directory": "docs/planning" })
}

#[test]
fn export_drift_reports_drift_through_the_real_filesystem() {
    let scratch = TempDir::new().expect("a scratch directory is available");
    let seed = scratch.path().join("kanban.seed");
    fs::create_dir_all(&seed).expect("the seed directory is created");
    let Wired { core, projects } = wired(scratch.path());
    create_project(&projects, seed.to_str().expect("the path is UTF-8"));
    author_planning_state(&core);

    core.command("export.render", &render("render-1"))
        .expect("the render applies");

    let clean = core
        .query("export.drift", &drift())
        .expect("the drift query serves");
    assert_eq!(clean["in_drift"], json!(false));
    assert_eq!(clean["entries"], json!([]));

    // A hand edit drifts.
    let spec_document = seed
        .join("docs")
        .join("planning")
        .join("specs")
        .join("CORE-S1.md");
    fs::write(&spec_document, "# hand edit\n").expect("the hand edit lands");
    let edited = core
        .query("export.drift", &drift())
        .expect("the drift query serves");
    assert_eq!(edited["in_drift"], json!(true));
    assert_eq!(
        edited["entries"],
        json!([{ "path": "specs/CORE-S1.md", "status": "differs" }])
    );

    // A deletion drifts as missing.
    fs::remove_file(
        seed.join("docs")
            .join("planning")
            .join("tickets")
            .join("CORE-T1.md"),
    )
    .expect("the ticket document leaves");
    let deleted = core
        .query("export.drift", &drift())
        .expect("the drift query serves");
    assert_eq!(
        deleted["entries"],
        json!([
            { "path": "specs/CORE-S1.md", "status": "differs" },
            { "path": "tickets/CORE-T1.md", "status": "missing" },
        ]),
        "entries report in path order with every status"
    );

    // An unexpected file drifts as unmatched.
    let stale = seed
        .join("docs")
        .join("planning")
        .join("plans")
        .join("CORE-P9.md");
    fs::create_dir_all(stale.parent().expect("the parent exists"))
        .expect("the stale directory is created");
    fs::write(&stale, "# a stale plan\n").expect("the stale document lands");
    let stale = core
        .query("export.drift", &drift())
        .expect("the drift query serves");
    assert_eq!(
        stale["entries"]
            .as_array()
            .expect("the entries are a list")
            .iter()
            .map(|entry| (entry["path"].clone(), entry["status"].clone()))
            .collect::<Vec<_>>(),
        vec![
            (json!("plans/CORE-P9.md"), json!("unmatched")),
            (json!("specs/CORE-S1.md"), json!("differs")),
            (json!("tickets/CORE-T1.md"), json!("missing")),
        ],
        "the unmatched file reports beside the other drift"
    );

    // A rerender clears everything the current state still holds.
    core.command("export.render", &render("render-2"))
        .expect("the rerender applies");
    let restored = core
        .query("export.drift", &drift())
        .expect("the drift query serves");
    assert_eq!(
        restored["entries"],
        json!([{ "path": "plans/CORE-P9.md", "status": "unmatched" }]),
        "a rerender restores every current artifact; the stale plan alone stays"
    );
}
