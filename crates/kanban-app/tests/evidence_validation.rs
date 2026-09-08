mod common;

use common::{harness, mutation};
use serde_json::json;

fn implementation_ticket(core: &kanban_app::Core) -> u64 {
    let spec = core
        .command(
            "spec.create",
            &json!({
                "mutation": mutation(0, "spec"),
                "project_id": 1,
                "content": {
                    "name": "Registration",
                    "short_description": "Versioned Plan graphs of Specs",
                    "problem_statement": "Planning must survive change without losing truth.",
                    "solution": "Immutable approved versions.",
                    "user_stories": "KAN-S3-US4",
                    "implementation_decisions": "Supersession is explicit.",
                    "testing_decisions": "Domain tests prove immutability.",
                    "out_of_scope": "The Ticket graph proposal.",
                    "further_notes": "None",
                },
            }),
        )
        .expect("the Spec authors");
    let created = core
        .command(
            "ticket.create",
            &json!({
                "mutation": mutation(0, "ticket"),
                "project_id": 1,
                "kind": "implementation",
                "priority": "normal",
                "spec_id": spec["id"],
                "slice": "Bind evidence to criteria",
                "criteria": [{
                    "outcome": "Reviewers validate evidence at one tip.",
                    "stories": ["CORE-S1-US1"]
                }],
            }),
        )
        .expect("the Ticket authors");
    created["id"].as_u64().expect("the identity is a number")
}

fn attach_repository(core: &kanban_app::Core, ticket: u64, key: &str) -> u64 {
    let item = core
        .command(
            "evidence.attach",
            &json!({
                "mutation": mutation(0, key),
                "project_id": 1,
                "entity_kind": "ticket",
                "entity_id": ticket.to_string(),
                "evidence_kind": "repository",
                "relative_path": "crates/kanban-app/src/evidence.rs",
                "commit_identity": "a".repeat(40),
            }),
        )
        .expect("evidence attaches");
    item["id"]
        .as_u64()
        .expect("the evidence identity is a number")
}

fn wired() -> common::DispatchHarness {
    let mut h = harness();
    let db = kanban_storage::Database::open(&h.database_path).unwrap();
    let attachments = h._dir.path().join("attachments");
    std::fs::create_dir_all(&attachments).unwrap();
    h.core
        .register_plans(
            std::sync::Arc::new(kanban_storage::SqlitePlanStore::new(&db)),
            std::sync::Arc::new(kanban_storage::SqliteProjectStore::new(&db)),
            std::sync::Arc::new(kanban_storage::SqliteSpecStore::new(&db)),
        )
        .unwrap();
    h.core
        .register_specs(
            std::sync::Arc::new(kanban_storage::SqliteSpecStore::new(&db)),
            std::sync::Arc::new(kanban_storage::SqliteProjectStore::new(&db)),
            std::sync::Arc::new(kanban_storage::SqlitePlanStore::new(&db)),
        )
        .unwrap();
    h.core
        .register_tickets(
            std::sync::Arc::new(kanban_storage::SqliteTicketStore::new(&db)),
            std::sync::Arc::new(kanban_storage::SqliteProjectStore::new(&db)),
            std::sync::Arc::new(kanban_storage::SqliteSpecStore::new(&db)),
            std::sync::Arc::new(kanban_storage::SqliteEvidenceStore::new(
                &db,
                attachments.clone(),
            )),
        )
        .unwrap();
    let evidence_store =
        std::sync::Arc::new(kanban_storage::SqliteEvidenceStore::new(&db, attachments));
    h.core
        .register_evidence(
            evidence_store.clone(),
            std::sync::Arc::new(kanban_storage::SqliteProjectStore::new(&db)),
        )
        .unwrap();
    h.core
        .register_criterion_bindings(
            std::sync::Arc::new(kanban_storage::SqliteCriterionBindingStore::new(&db)),
            std::sync::Arc::new(kanban_storage::SqliteTicketStore::new(&db)),
            evidence_store,
        )
        .unwrap();
    h
}

fn bound_criterion(core: &kanban_app::Core) -> (u64, u64) {
    let ticket = implementation_ticket(core);
    let evidence = attach_repository(core, ticket, "proof");
    let bound = core
        .command(
            "criterion.evidence.attach",
            &json!({
                "mutation": mutation(0, "bind"),
                "ticket_id": ticket,
                "criterion_index": 0,
                "evidence_id": evidence,
                "tip": "a".repeat(40),
            }),
        )
        .expect("implementers attach evidence to every Acceptance Criterion");
    (ticket, bound["evidence_id"].as_u64().unwrap())
}

#[test]
fn evidence_validation_binds_implementer_evidence_to_a_criterion() {
    let h = wired();
    let (ticket, evidence) = bound_criterion(&h.core);
    let listed = h
        .core
        .query("criterion.bindings", &json!({"ticket_id": ticket}))
        .unwrap();
    assert_eq!(listed["bindings"][0]["evidence_id"], evidence);
    assert_eq!(listed["bindings"][0]["review"], "pending");
    assert_eq!(listed["bindings"][0]["satisfied"], false);
}

#[test]
fn evidence_validation_requires_reviewer_validation_before_satisfaction() {
    let h = wired();
    let (ticket, _) = bound_criterion(&h.core);
    let refused = h
        .core
        .command(
            "criterion.satisfy",
            &json!({
                "mutation": mutation(0, "too-soon"),
                "ticket_id": ticket,
                "criterion_index": 0,
                "tip": "a".repeat(40),
            }),
        )
        .expect_err("unvalidated evidence cannot satisfy a criterion");
    assert_eq!(refused.code, kanban_dto::ErrorCode::InvalidRequest);
}

#[test]
fn evidence_validation_satisfies_only_the_approved_tip() {
    let h = wired();
    let (ticket, _) = bound_criterion(&h.core);
    h.core
        .command(
            "criterion.evidence.review",
            &json!({
                "mutation": mutation(0, "validate"),
                "ticket_id": ticket,
                "criterion_index": 0,
                "review": "validated",
            }),
        )
        .unwrap();
    let satisfied = h
        .core
        .command(
            "criterion.satisfy",
            &json!({
                "mutation": mutation(0, "approve"),
                "ticket_id": ticket,
                "criterion_index": 0,
                "tip": "a".repeat(40),
            }),
        )
        .unwrap();
    assert_eq!(satisfied["satisfied"], true);
    assert!(
        h.core
            .command(
                "criterion.satisfy",
                &json!({
                    "mutation": mutation(0, "wrong-tip"),
                    "ticket_id": ticket,
                    "criterion_index": 0,
                    "tip": "b".repeat(40),
                }),
            )
            .is_err(),
        "a different tip cannot satisfy the bound criterion"
    );
}

#[test]
fn evidence_validation_voids_outstanding_approvals_on_content_change() {
    let h = wired();
    let (ticket, _) = bound_criterion(&h.core);
    h.core
        .command(
            "criterion.evidence.review",
            &json!({
                "mutation": mutation(0, "validate"),
                "ticket_id": ticket,
                "criterion_index": 0,
                "review": "validated",
            }),
        )
        .unwrap();
    h.core
        .command(
            "criterion.satisfy",
            &json!({
                "mutation": mutation(0, "approve"),
                "ticket_id": ticket,
                "criterion_index": 0,
                "tip": "a".repeat(40),
            }),
        )
        .unwrap();
    let voided = h
        .core
        .command(
            "criterion.invalidate",
            &json!({
                "mutation": mutation(0, "changed"),
                "ticket_id": ticket,
                "observed_tip": "b".repeat(40),
            }),
        )
        .unwrap();
    assert_eq!(voided["bindings"][0]["void"], true);
    assert_eq!(voided["bindings"][0]["satisfied"], false);
}

#[test]
fn evidence_validation_lets_humans_complete_task_criteria() {
    let h = wired();
    let created = h
        .core
        .command(
            "ticket.create",
            &json!({
                "mutation": mutation(0, "task"),
                "project_id": 1,
                "kind": "task",
                "priority": "normal",
                "title": "Archive the register",
                "subtype": "administrative",
                "mode": "human",
                "completion": ["The old register is archived and restorable."],
            }),
        )
        .unwrap();
    let ticket = created["id"].as_u64().unwrap();
    let completed = h
        .core
        .command(
            "criterion.complete",
            &json!({
                "mutation": mutation(0, "done"),
                "ticket_id": ticket,
                "criterion_index": 0,
            }),
        )
        .unwrap();
    assert_eq!(completed["kind"], "task");
    assert_eq!(completed["satisfied"], true);
}

#[test]
fn evidence_validation_refuses_implementation_review_without_satisfied_bindings() {
    let mut h = wired();
    let db = kanban_storage::Database::open(&h.database_path).unwrap();
    h.core
        .register_lifecycle(
            std::sync::Arc::new(kanban_storage::SqliteTicketStore::new(&db)),
            std::sync::Arc::new(kanban_storage::SqliteDependencyStore::new(&db)),
            std::sync::Arc::new(kanban_storage::SqliteProjectStore::new(&db)),
            std::sync::Arc::new(kanban_storage::SqliteScheduleStore::new(&db)),
            Some(std::sync::Arc::new(
                kanban_storage::SqliteCriterionBindingStore::new(&db),
            )),
        )
        .unwrap();
    let ticket = implementation_ticket(&h.core);
    let current = h
        .core
        .query("ticket.get", &json!({ "ticket_id": ticket }))
        .unwrap();
    let error = h
        .core
        .command(
            "ticket.review",
            &json!({
                "mutation": mutation(current["version"].as_u64().unwrap(), "review"),
                "ticket_id": ticket,
                "decision": "approve",
            }),
        )
        .expect_err("approval without bound evidence is refused");
    assert_eq!(error.code, kanban_dto::ErrorCode::InvalidRequest);
}
