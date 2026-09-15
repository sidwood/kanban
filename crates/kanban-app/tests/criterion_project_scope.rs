//! App gate for criterion event scope (KAN-T138-AC4, KAN-S2-US1,
//! DR-AE-01): every criterion completion, binding, review,
//! satisfaction, and invalidation event lands on the owning Ticket's
//! Project timeline, in a database holding more than one Project.

use std::sync::Arc;

use kanban_app::{Core, ProjectStore, TimelineEnvelope};
use kanban_domain::ProjectRegistration;
use kanban_dto::{ErrorCode, TimelineEntityKind, TimelineEntityRef, TimelineEventKind};
use kanban_storage::{
    SqliteCriterionBindingStore, SqliteEvidenceStore, SqliteProjectStore, SqliteSpecStore,
    SqliteTicketStore,
};
use serde_json::{Value, json};

mod common;

use common::{DispatchHarness, harness, mutation};

const TIP_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const TIP_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

/// The dispatch harness with a second Project seeded and the Ticket,
/// evidence, and criterion binding operations registered.
fn two_projects() -> DispatchHarness {
    let mut h = harness();
    let projects = SqliteProjectStore::new(&h.database);
    let registration = ProjectRegistration::new(
        "WAVE",
        "Wave pool",
        "/repositories/wave",
        "/workspaces/wave.seed",
        "main",
        "wave.seed",
        Some("wave-main"),
        None,
    )
    .expect("the second registration validates");
    projects
        .create(&registration, &|id| {
            TimelineEnvelope::project(
                id.value(),
                TimelineEventKind::Transition,
                Some(TimelineEntityRef {
                    kind: TimelineEntityKind::Project,
                    id: id.value().to_string(),
                }),
                json!({ "action": "registered" }),
            )
        })
        .expect("the second Project lands");
    let attachments = h._dir.path().join("attachments");
    std::fs::create_dir_all(&attachments).expect("the attachment directory exists");
    let tickets = Arc::new(SqliteTicketStore::new(&h.database));
    let projects = Arc::new(SqliteProjectStore::new(&h.database));
    let evidence = Arc::new(SqliteEvidenceStore::new(&h.database, attachments));
    h.core
        .register_tickets(
            tickets.clone(),
            projects.clone(),
            Arc::new(SqliteSpecStore::new(&h.database)),
            evidence.clone(),
        )
        .expect("the ticket operations register");
    h.core
        .register_evidence(evidence.clone(), projects)
        .expect("the evidence operations register");
    h.core
        .register_criterion_bindings(
            Arc::new(SqliteCriterionBindingStore::new(&h.database)),
            tickets,
            evidence,
            Arc::new(kanban_storage::SqliteReviewExecutionStore::new(&h.database)),
        )
        .expect("the criterion operations register");
    h
}

fn id(record: &Value) -> u64 {
    record["id"]
        .as_u64()
        .expect("the record carries an identity")
}

/// Author one Task in `project` with two completion criteria.
fn task_in(core: &Core, project: u64, key: &str) -> u64 {
    let created = core
        .command(
            "ticket.create",
            &json!({
                "mutation": mutation(0, format!("{key}-task")),
                "project_id": project,
                "kind": "task",
                "priority": "normal",
                "title": "Bounded operational work",
                "subtype": "operational",
                "mode": "human",
                "completion": ["The work is done.", "The work is verified."],
            }),
        )
        .expect("the Task authors");
    id(&created)
}

/// Every evidence-kind timeline row about `ticket`, as `(scope,
/// project_id, action)` in append order.
fn evidence_events(path: &std::path::Path, ticket: u64) -> Vec<(String, String, String)> {
    let conn = rusqlite::Connection::open(path).expect("the database reopens");
    conn.prepare(
        "SELECT scope, project_id, json_extract(detail, '$.action')
         FROM timeline_events
         WHERE kind = 'evidence' AND entity_kind = 'ticket' AND entity_id = ?1
         ORDER BY id",
    )
    .expect("the statement prepares")
    .query_map(rusqlite::params![ticket.to_string()], |row| {
        Ok((row.get(0)?, row.get(1)?, row.get(2)?))
    })
    .expect("the rows serve")
    .collect::<Result<Vec<_>, _>>()
    .expect("the rows decode")
}

fn complete(core: &Core, ticket: u64, index: u64, key: &str) -> Value {
    core.command(
        "criterion.complete",
        &json!({
            "mutation": mutation(0, key),
            "ticket_id": ticket,
            "criterion_index": index,
        }),
    )
    .expect("the completion lands")
}

#[test]
fn criterion_completion_lands_on_the_owning_projects_timeline() {
    let h = two_projects();
    let first = task_in(&h.core, 1, "first");
    let second = task_in(&h.core, 2, "second");

    let completed = complete(&h.core, second, 0, "complete-second");
    assert_eq!(completed["satisfied"], json!(true));
    assert_eq!(
        evidence_events(&h.database_path, second),
        vec![("project".to_owned(), "2".to_owned(), "completed".to_owned())],
        "Project 2 owns the Task, so Project 2 records its completion"
    );

    complete(&h.core, first, 0, "complete-first");
    assert_eq!(
        evidence_events(&h.database_path, first),
        vec![("project".to_owned(), "1".to_owned(), "completed".to_owned())]
    );
}

#[test]
fn criterion_binding_review_satisfaction_and_invalidation_scope_to_the_owning_project() {
    let h = two_projects();
    let (ticket, submission) = common::review::prepare_on_in(&h.core, &h.database_path, 2, 1);
    let evidence = h
        .core
        .command(
            "evidence.attach",
            &json!({
                "mutation": mutation(0, "bound-evidence"),
                "project_id": 2,
                "entity_kind": "ticket",
                "entity_id": ticket.to_string(),
                "evidence_kind": "repository",
                "relative_path": "crates/kanban-app/src/tip_binding.rs",
                "commit_identity": TIP_A,
            }),
        )
        .expect("the evidence attaches");

    h.core
        .command(
            "criterion.evidence.attach",
            &json!({
                "mutation": mutation(0, "bound-attach"),
                "ticket_id": ticket,
                "criterion_index": 0,
                "evidence_id": id(&evidence),
                "tip": TIP_A,
            }),
        )
        .expect("the binding lands");
    h.core
        .command(
            "criterion.evidence.review",
            &json!({
                "mutation": mutation(0, "bound-review"),
                "ticket_id": ticket,
                "criterion_index": 0,
                "review": "validated",
            }),
        )
        .expect("the review lands");
    let review = common::review::start(&h.core, ticket, &submission, "bound-review-start");
    common::review::approve_required_stage(&h.core, &review, "bound-review-approve");
    h.core
        .command(
            "criterion.satisfy",
            &json!({
                "mutation": mutation(0, "bound-satisfy"),
                "ticket_id": ticket,
                "criterion_index": 0,
                "tip": TIP_A,
            }),
        )
        .expect("the satisfaction lands");
    h.core
        .command(
            "criterion.invalidate",
            &json!({
                "mutation": mutation(0, "bound-invalidate"),
                "ticket_id": ticket,
                "observed_tip": TIP_B,
            }),
        )
        .expect("the invalidation lands");

    assert_eq!(
        evidence_events(&h.database_path, ticket),
        vec![
            ("project".to_owned(), "2".to_owned(), "attached".to_owned()),
            ("project".to_owned(), "2".to_owned(), "bound".to_owned()),
            ("project".to_owned(), "2".to_owned(), "reviewed".to_owned()),
            ("project".to_owned(), "2".to_owned(), "satisfied".to_owned()),
            (
                "project".to_owned(),
                "2".to_owned(),
                "invalidated".to_owned()
            ),
        ],
        "every criterion event names the owning Project, never Project 1"
    );
}

#[test]
fn criterion_commands_refuse_an_unknown_ticket_without_recording_anything() {
    let h = two_projects();
    let before: i64 = rusqlite::Connection::open(&h.database_path)
        .expect("the database reopens")
        .query_row(
            "SELECT COUNT(*) FROM timeline_events WHERE kind = 'evidence'",
            [],
            |row| row.get(0),
        )
        .expect("the count serves");

    for (operation, payload) in [
        (
            "criterion.evidence.review",
            json!({
                "mutation": mutation(0, "unknown-review"),
                "ticket_id": 999,
                "criterion_index": 0,
                "review": "validated",
            }),
        ),
        (
            "criterion.satisfy",
            json!({
                "mutation": mutation(0, "unknown-satisfy"),
                "ticket_id": 999,
                "criterion_index": 0,
                "tip": TIP_A,
            }),
        ),
        (
            "criterion.invalidate",
            json!({
                "mutation": mutation(0, "unknown-invalidate"),
                "ticket_id": 999,
                "observed_tip": TIP_B,
            }),
        ),
    ] {
        let error = h
            .core
            .command(operation, &payload)
            .expect_err("an unknown Ticket is refused");
        assert_eq!(error.code, ErrorCode::NotFound, "{operation}: {error:?}");
    }

    let after: i64 = rusqlite::Connection::open(&h.database_path)
        .expect("the database reopens")
        .query_row(
            "SELECT COUNT(*) FROM timeline_events WHERE kind = 'evidence'",
            [],
            |row| row.get(0),
        )
        .expect("the count serves");
    assert_eq!(after, before, "a refusal appends no evidence event");
}
