//! App gate for the Herdr telemetry projection (KAN-T40): push
//! events append to the timeline as telemetry while workflow state
//! stays untouched, whatever the events claim.

use std::collections::BTreeMap;
use std::sync::Arc;

use kanban_app::catalog::exposed_operations;
use kanban_app::dispatch::Core;
use kanban_app::herdr::NoopHerdrProjectObserver;
use kanban_app::project::ProjectStore;
use kanban_app::telemetry::{TelemetryProjection, project_herdr_event};
use kanban_app::timeline::{TimelineEnvelope, TimelineStore};
use kanban_dto::{
    TimelineEntityKind, TimelineEntityRef, TimelineEventKind, TimelineQuery, TimelineScope,
};
use kanban_service::timeline::StorageTimelineStore;
use kanban_storage::{
    AllowAllMigrations, Database, RetentionPolicy, SqliteHerdrSettingsStore,
    SqliteIdempotencyStore, SqliteInitiativeStore, SqlitePlanStore, SqliteProjectStore,
    SqliteSpecStore,
};
use serde_json::{Value, json};
use tempfile::TempDir;

struct Wired {
    core: Core,
    projects: Arc<SqliteProjectStore>,
    database: Arc<Database>,
}

fn wired(scratch: &TempDir) -> Wired {
    let mut database =
        Database::open(&scratch.path().join("kanban.sqlite")).expect("a scratch database opens");
    database
        .migrate(&AllowAllMigrations)
        .expect("the migrations apply");
    let database = Arc::new(database);
    let projects = Arc::new(SqliteProjectStore::new(&database));
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
        Arc::new(kanban_service::LocalRepositories),
        initiatives,
        Arc::new(SqliteHerdrSettingsStore::new(&database)),
        Arc::new(NoopHerdrProjectObserver),
    )
    .expect("the project operations register");
    core.register_specs(
        Arc::new(SqliteSpecStore::new(&database)),
        projects.clone(),
        Arc::new(SqlitePlanStore::new(&database)),
    )
    .expect("the spec operations register");
    Wired {
        core,
        projects,
        database,
    }
}

fn create_project(projects: &SqliteProjectStore, repository: &str) {
    let registration = kanban_domain::ProjectRegistration::new(
        "CORE",
        "Control plane",
        repository,
        "/workspaces/kanban.seed",
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

fn spec_content(name: &str) -> Value {
    json!({
        "name": name,
        "short_description": "Observation",
        "problem_statement": "Kanban cannot see Herdr",
        "solution": "Telemetry",
        "user_stories": "- US2",
        "implementation_decisions": "Projection only",
        "testing_decisions": "Append and invariant tests",
        "out_of_scope": "Submission intake",
        "further_notes": ""
    })
}

fn create_spec(core: &Core, project_id: u64, key: &str) {
    core.command(
        "spec.create",
        &json!({
            "mutation": { "optimistic_version": 0, "idempotency_key": key },
            "project_id": project_id,
            "content": spec_content("Herdr observation"),
        }),
    )
    .expect("the spec creates");
}

/// Project `events` for `project_id`, appending every timeline row
/// the projection emits — the same write the service observer makes.
fn project_onto_timeline(database: &Database, project_id: u64, events: &[Value]) {
    for event in events {
        for projection in project_herdr_event(project_id, event) {
            match projection {
                TelemetryProjection::Timeline(envelope) => database
                    .append_timeline_event(&envelope)
                    .expect("the telemetry row appends"),
                TelemetryProjection::Attention(signal) => {
                    panic!("no push event raises an attention signal in this slice: {signal:?}")
                }
            }
        }
    }
}

fn timeline(database: &Arc<Database>) -> Vec<(TimelineEventKind, Value)> {
    StorageTimelineStore::new(database.clone())
        .query(&TimelineQuery {
            scope: TimelineScope::Project(1),
            entity: None,
            kinds: None,
            since: None,
            until: None,
        })
        .expect("the timeline serves")
        .into_iter()
        .map(|event| (event.kind, event.detail))
        .collect()
}

fn kinds_by_count(rows: &[(TimelineEventKind, Value)]) -> BTreeMap<String, usize> {
    let mut counts = BTreeMap::new();
    for (kind, _) in rows {
        *counts.entry(kind.as_str().to_owned()).or_insert(0) += 1;
    }
    counts
}

fn role_opened() -> Value {
    json!({
        "kind": "role.opened",
        "role": "implementer",
        "project": "CORE",
        "ticket": "KAN-T40",
        "lane": "in_progress",
        "reviewer_slot": "primary",
        "run": "run-1",
        "harness": "claude-code",
        "model": "opus-5"
    })
}

mod telemetry_projection {
    use super::*;

    #[test]
    fn push_events_append_to_the_timeline_as_telemetry() {
        let scratch = TempDir::new().expect("a scratch directory is available");
        let wired = wired(&scratch);
        create_project(&wired.projects, scratch.path().to_str().unwrap());

        project_onto_timeline(
            &wired.database,
            1,
            &[
                role_opened(),
                json!({ "kind": "role.output", "role": "implementer", "text": "working" }),
            ],
        );

        let rows = timeline(&wired.database);
        let kinds: Vec<&str> = rows.iter().map(|(kind, _)| kind.as_str()).collect();
        assert_eq!(
            kinds,
            vec!["transition", "telemetry", "telemetry"],
            "push events append after the registration row, as telemetry and nothing else"
        );
        let (_, opened) = &rows[1];
        assert_eq!(opened["source"], json!("herdr"));
        assert_eq!(opened["event"], json!("role.opened"));
        assert_eq!(opened["role"], json!("implementer"));
        assert_eq!(
            opened["payload"],
            role_opened(),
            "the payload rides unchanged"
        );
    }

    #[test]
    fn role_tab_metadata_rides_the_telemetry_row() {
        let scratch = TempDir::new().expect("a scratch directory is available");
        let wired = wired(&scratch);
        create_project(&wired.projects, scratch.path().to_str().unwrap());

        project_onto_timeline(
            &wired.database,
            1,
            &[
                role_opened(),
                json!({
                    "kind": "role.opened",
                    "role": "reviewer",
                    "project": "CORE",
                    "ticket": "KAN-T40",
                    "lane": "review"
                }),
            ],
        );

        let rows = timeline(&wired.database);
        assert_eq!(
            rows[1].1["tab"],
            json!({
                "project": "CORE",
                "ticket": "KAN-T40",
                "lane": "in_progress",
                "reviewer_slot": "primary",
                "run": "run-1",
                "harness": "claude-code",
                "model": "opus-5"
            }),
            "the tab carries Project, Ticket, Lane, reviewer slot, run, harness, and model"
        );
        assert_eq!(
            rows[2].1["tab"],
            json!({ "project": "CORE", "ticket": "KAN-T40", "lane": "review" }),
            "metadata Herdr did not report is absent, not invented"
        );
    }

    #[test]
    fn exits_disconnects_and_stalls_leave_workflow_state_untouched() {
        let scratch = TempDir::new().expect("a scratch directory is available");
        let wired = wired(&scratch);
        create_project(&wired.projects, scratch.path().to_str().unwrap());
        create_spec(&wired.core, 1, "spec-1");

        let project_before = wired
            .projects
            .find(kanban_domain::ProjectId::new(1))
            .expect("the project loads")
            .expect("the project exists");
        let rows_before = timeline(&wired.database);
        let hostile = vec![
            json!({ "kind": "role.exited", "role": "implementer", "exit_code": 0 }),
            json!({ "kind": "session.disconnected" }),
            json!({ "kind": "role.stalled", "role": "implementer" }),
            json!({ "kind": "run.passed", "ticket": "KAN-T40", "verdict": "pass" }),
            json!({ "kind": "spec.version.approved", "spec_id": 1, "version": 2 }),
            json!({ "kind": "ticket.transition", "ticket": "KAN-T40", "to": "done" }),
            json!({ "kind": "lane.moved", "lane": "landed" }),
        ];

        project_onto_timeline(&wired.database, 1, &hostile);

        let project_after = wired
            .projects
            .find(kanban_domain::ProjectId::new(1))
            .expect("the project loads")
            .expect("the project exists");
        assert_eq!(
            project_after.version(),
            project_before.version(),
            "telemetry never moves the Project's version"
        );
        assert_eq!(
            project_after.is_archived(),
            project_before.is_archived(),
            "telemetry never archives a Project"
        );

        let rows_after = timeline(&wired.database);
        let mut expected_before = kinds_by_count(&rows_before);
        *expected_before.entry("telemetry".to_owned()).or_insert(0) += hostile.len();
        assert_eq!(
            kinds_by_count(&rows_after),
            expected_before,
            "the flood of exits, disconnects, stalls, and verdict-shaped events adds telemetry rows only"
        );

        let listed = wired
            .core
            .query("spec.list", &json!({ "project_id": 1 }))
            .expect("the spec surface still serves");
        assert_eq!(
            listed["specs"][0]["execution"],
            json!("unplanned"),
            "telemetry never moves execution state"
        );
        assert_eq!(
            listed["specs"][0]["version"],
            json!(1),
            "telemetry never mints a content version"
        );
    }
}
