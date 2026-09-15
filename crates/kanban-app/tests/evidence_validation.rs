mod common;

use std::collections::HashMap;
use std::sync::Arc;

use common::{harness, mutation};
use kanban_app::{WorkspaceGitObserver, WorkspaceGitSnapshot};
use kanban_domain::WorkspaceCheckout;
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
            std::sync::Arc::new(kanban_storage::SqliteReviewExecutionStore::new(&db)),
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
    let (h, ticket, submission) = bind_on_prepared(&"a".repeat(40));
    validate_bound_evidence(&h.core, ticket, "validate");
    let review = common::review::start(&h.core, ticket, &submission, "approve");
    common::review::approve_required_stage(&h.core, &review, "approve");
    let satisfied = satisfy(&h.core, ticket, &"a".repeat(40), "approve-satisfy")
        .expect("a completed required-stage review at the bound tip satisfies");
    assert_eq!(satisfied["satisfied"], true);
    assert!(
        satisfy(&h.core, ticket, &"b".repeat(40), "wrong-tip").is_err(),
        "a different tip cannot satisfy the bound criterion"
    );
}

#[test]
fn evidence_validation_voids_outstanding_approvals_on_content_change() {
    let (h, ticket, submission) = bind_on_prepared(&"a".repeat(40));
    validate_bound_evidence(&h.core, ticket, "validate");
    let review = common::review::start(&h.core, ticket, &submission, "approve");
    common::review::approve_required_stage(&h.core, &review, "approve");
    satisfy(&h.core, ticket, &"a".repeat(40), "approve-satisfy")
        .expect("a completed required-stage review at the bound tip satisfies");
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

fn validate_bound_evidence(core: &kanban_app::Core, ticket: u64, key: &str) {
    core.command(
        "criterion.evidence.review",
        &json!({
            "mutation": mutation(0, key),
            "ticket_id": ticket,
            "criterion_index": 0,
            "review": "validated",
        }),
    )
    .expect("reviewers validate attached evidence");
}

fn satisfy(
    core: &kanban_app::Core,
    ticket: u64,
    tip: &str,
    key: &str,
) -> Result<serde_json::Value, kanban_dto::ApiError> {
    core.command(
        "criterion.satisfy",
        &json!({
            "mutation": mutation(0, key),
            "ticket_id": ticket,
            "criterion_index": 0,
            "tip": tip,
        }),
    )
}

fn bind_on_prepared(tip: &str) -> (common::DispatchHarness, u64, serde_json::Value) {
    let (mut h, ticket, submission) = common::review::prepared();
    let attachments = h._dir.path().join("attachments");
    std::fs::create_dir_all(&attachments).unwrap();
    let db = kanban_storage::Database::open(&h.database_path).unwrap();
    let evidence_store = Arc::new(kanban_storage::SqliteEvidenceStore::new(
        &db,
        attachments.clone(),
    ));
    h.core
        .register_tickets(
            Arc::new(kanban_storage::SqliteTicketStore::new(&db)),
            Arc::new(kanban_storage::SqliteProjectStore::new(&db)),
            Arc::new(kanban_storage::SqliteSpecStore::new(&db)),
            Arc::new(kanban_storage::SqliteEvidenceStore::new(&db, attachments)),
        )
        .unwrap();
    h.core
        .register_evidence(
            evidence_store.clone(),
            Arc::new(kanban_storage::SqliteProjectStore::new(&db)),
        )
        .unwrap();
    h.core
        .register_criterion_bindings(
            Arc::new(kanban_storage::SqliteCriterionBindingStore::new(&db)),
            Arc::new(kanban_storage::SqliteTicketStore::new(&db)),
            evidence_store,
            Arc::new(kanban_storage::SqliteReviewExecutionStore::new(&db)),
        )
        .unwrap();
    let evidence = h
        .core
        .command(
            "evidence.attach",
            &json!({
                "mutation": mutation(0, "proof"),
                "project_id": 1,
                "entity_kind": "ticket",
                "entity_id": ticket.to_string(),
                "evidence_kind": "repository",
                "relative_path": "crates/kanban-app/src/evidence.rs",
                "commit_identity": tip,
            }),
        )
        .expect("evidence attaches");
    h.core
        .command(
            "criterion.evidence.attach",
            &json!({
                "mutation": mutation(0, "bind"),
                "ticket_id": ticket,
                "criterion_index": 0,
                "evidence_id": evidence["id"],
                "tip": tip,
            }),
        )
        .expect("implementers attach evidence to the criterion");
    (h, ticket, submission)
}

#[test]
fn evidence_validation_refuses_satisfaction_without_a_review_execution() {
    let h = wired();
    let (ticket, _) = bound_criterion(&h.core);
    validate_bound_evidence(&h.core, ticket, "validate");
    let refused = satisfy(&h.core, ticket, &"a".repeat(40), "absent-review")
        .expect_err("a per-criterion evidence flag cannot satisfy without a completed review");
    assert_eq!(refused.code, kanban_dto::ErrorCode::InvalidRequest);
}

#[test]
fn evidence_validation_refuses_satisfaction_when_review_is_incomplete() {
    let (h, ticket, submission) = bind_on_prepared(&"a".repeat(40));
    validate_bound_evidence(&h.core, ticket, "validate");
    common::review::start(&h.core, ticket, &submission, "incomplete");
    let refused = satisfy(&h.core, ticket, &"a".repeat(40), "incomplete-satisfy")
        .expect_err("an incomplete review cannot satisfy a criterion");
    assert_eq!(refused.code, kanban_dto::ErrorCode::InvalidRequest);
}

#[test]
fn evidence_validation_refuses_satisfaction_when_review_is_rejected() {
    let (h, ticket, submission) = bind_on_prepared(&"a".repeat(40));
    validate_bound_evidence(&h.core, ticket, "validate");
    let review = common::review::start(&h.core, ticket, &submission, "reject");
    common::review::reject_required_stage(&h.core, &review, "reject");
    let refused = satisfy(&h.core, ticket, &"a".repeat(40), "rejected-satisfy")
        .expect_err("a rejected review cannot satisfy a criterion");
    assert_eq!(refused.code, kanban_dto::ErrorCode::InvalidRequest);
}

#[test]
fn evidence_validation_refuses_satisfaction_when_review_is_expired() {
    let (h, ticket, submission) = bind_on_prepared(&"a".repeat(40));
    validate_bound_evidence(&h.core, ticket, "validate");
    let review = common::review::start(&h.core, ticket, &submission, "expire");
    h.core
        .command(
            "review.expire",
            &json!({
                "mutation": mutation(review["version"].as_u64().unwrap(), "expire-review"),
                "review_id": review["id"],
            }),
        )
        .expect("an in-progress review can expire");
    let refused = satisfy(&h.core, ticket, &"a".repeat(40), "expired-satisfy")
        .expect_err("an expired review cannot satisfy a criterion");
    assert_eq!(refused.code, kanban_dto::ErrorCode::InvalidRequest);
}

#[test]
fn evidence_validation_refuses_satisfaction_when_review_is_at_the_wrong_tip() {
    let (h, ticket, submission) = bind_on_prepared(&"b".repeat(40));
    validate_bound_evidence(&h.core, ticket, "validate");
    let review = common::review::start(&h.core, ticket, &submission, "wrong-tip");
    common::review::approve_required_stage(&h.core, &review, "wrong-tip");
    let refused = satisfy(&h.core, ticket, &"b".repeat(40), "wrong-tip-satisfy")
        .expect_err("a review at a different tip cannot satisfy the bound criterion");
    assert_eq!(refused.code, kanban_dto::ErrorCode::InvalidRequest);
}

struct FixedHead {
    snapshots: HashMap<String, WorkspaceGitSnapshot>,
}

impl WorkspaceGitObserver for FixedHead {
    fn observe(&self, workspace_path: &str, _repository_path: &str) -> WorkspaceGitSnapshot {
        self.snapshots
            .get(workspace_path)
            .cloned()
            .unwrap_or(WorkspaceGitSnapshot {
                present: false,
                ..WorkspaceGitSnapshot::default()
            })
    }
}

fn snapshot(
    head: Option<&str>,
    tree: Option<&str>,
    working_tree_clean: Option<bool>,
) -> WorkspaceGitSnapshot {
    WorkspaceGitSnapshot {
        present: true,
        repository_identity: Some("identity".to_owned()),
        checkout: Some(WorkspaceCheckout::Branch("feature".to_owned())),
        head: head.map(str::to_owned),
        working_tree_clean,
        unique_unlanded_commits: Some(false),
        tree: tree.map(str::to_owned),
    }
}

fn listed_binding(core: &kanban_app::Core, ticket: u64) -> serde_json::Value {
    core.query("criterion.bindings", &json!({ "ticket_id": ticket }))
        .unwrap()["bindings"][0]
        .clone()
}

fn satisfied_at_tip_a() -> (common::DispatchHarness, u64) {
    let (h, ticket, submission) = bind_on_prepared(&"a".repeat(40));
    validate_bound_evidence(&h.core, ticket, "validate");
    let review = common::review::start(&h.core, ticket, &submission, "observe-review");
    common::review::approve_required_stage(&h.core, &review, "observe-review");
    satisfy(&h.core, ticket, &"a".repeat(40), "observe-satisfy")
        .expect("a completed required-stage review at the bound tip satisfies");
    (h, ticket)
}

fn seat_and_observe(
    h: &mut common::DispatchHarness,
    ticket: u64,
    observed: WorkspaceGitSnapshot,
    path: &str,
    key: &str,
) {
    let db = kanban_storage::Database::open(&h.database_path).unwrap();
    let projects = Arc::new(kanban_storage::SqliteProjectStore::new(&db));
    let workspaces = Arc::new(kanban_storage::SqliteWorkspaceStore::new(&db));
    let tickets = Arc::new(kanban_storage::SqliteTicketStore::new(&db));
    h.core
        .register_workspaces(
            workspaces.clone(),
            projects.clone(),
            Arc::new(FixedHead {
                snapshots: HashMap::from([(path.to_owned(), observed)]),
            }),
        )
        .unwrap();
    h.core
        .register_lanes(
            Arc::new(kanban_storage::SqliteLaneStore::new(&db)),
            projects,
            workspaces,
            tickets,
        )
        .unwrap();
    let workspace = h
        .core
        .command(
            "workspace.register",
            &json!({
                "mutation": mutation(0, format!("{key}-workspace")),
                "project_id": 1,
                "path": path,
            }),
        )
        .expect("the execution Workspace registers");
    let listed_lanes = h
        .core
        .query("lane.list", &json!({ "project_id": 1 }))
        .expect("the Lane listing serves");
    let lane = listed_lanes["lanes"]
        .as_array()
        .expect("the listing carries Lanes")
        .iter()
        .find(|lane| lane["ticket_id"] == ticket)
        .expect("the prepared Ticket already occupies a Lane")
        .clone();
    h.core
        .command(
            "lane.workspace.assign",
            &json!({
                "mutation": mutation(lane["version"].as_u64().unwrap(), format!("{key}-assign")),
                "lane_id": lane["id"],
                "workspace_id": workspace["id"],
            }),
        )
        .expect("the Lane claims the Workspace");
    let listed_workspaces = h
        .core
        .query("workspace.list", &json!({ "project_id": 1 }))
        .expect("the Workspace listing serves");
    let workspace_version = listed_workspaces["workspaces"][0]["version"]
        .as_u64()
        .expect("the assigned Workspace carries a version");
    h.core
        .command(
            "workspace.observe",
            &json!({
                "mutation": mutation(workspace_version, format!("{key}-observe")),
                "workspace_id": workspace["id"],
            }),
        )
        .expect("the production observer reads the workspace");
}

fn evidence_id(core: &kanban_app::Core, ticket: u64) -> u64 {
    listed_binding(core, ticket)["evidence_id"]
        .as_u64()
        .expect("a bound criterion carries evidence")
}

#[test]
fn evidence_validation_voids_outstanding_approvals_when_observed_head_changes() {
    let (mut h, ticket) = satisfied_at_tip_a();
    seat_and_observe(
        &mut h,
        ticket,
        snapshot(Some(&"b".repeat(40)), None, Some(true)),
        "/workspaces/kanban.feature",
        "observe",
    );
    let current = listed_binding(&h.core, ticket);
    assert_eq!(
        current["void"], true,
        "a content change must void outstanding approvals without criterion.invalidate"
    );
    assert_eq!(current["satisfied"], false);
}

#[test]
fn evidence_validation_keeps_a_commit_bound_approval_when_clean_head_is_unchanged() {
    let (mut h, ticket) = satisfied_at_tip_a();
    seat_and_observe(
        &mut h,
        ticket,
        snapshot(Some(&"a".repeat(40)), Some(&"c".repeat(40)), Some(true)),
        "/workspaces/kanban.clean",
        "clean-unchanged",
    );
    let current = listed_binding(&h.core, ticket);
    assert_eq!(
        current["void"], false,
        "clean unchanged commit-bound content must keep the approval"
    );
    assert_eq!(current["satisfied"], true);
}

#[test]
fn evidence_validation_keeps_a_tree_bound_approval_when_the_observed_tree_is_unchanged() {
    let (mut h, ticket) = satisfied_at_tip_a();
    seat_and_observe(
        &mut h,
        ticket,
        snapshot(Some(&"b".repeat(40)), Some(&"a".repeat(40)), Some(true)),
        "/workspaces/kanban.tree",
        "tree-unchanged",
    );
    let current = listed_binding(&h.core, ticket);
    assert_eq!(
        current["void"], false,
        "a tree-hash-bound approval must survive observation of the commit that owns that tree"
    );
    assert_eq!(current["satisfied"], true);
}

#[test]
fn evidence_validation_voids_outstanding_approvals_when_same_head_is_dirty() {
    let (mut h, ticket) = satisfied_at_tip_a();
    seat_and_observe(
        &mut h,
        ticket,
        snapshot(Some(&"a".repeat(40)), Some(&"a".repeat(40)), Some(false)),
        "/workspaces/kanban.dirty",
        "dirty-same-head",
    );
    let current = listed_binding(&h.core, ticket);
    assert_eq!(
        current["void"], true,
        "dirty content at the same HEAD must void outstanding approvals"
    );
    assert_eq!(current["satisfied"], false);
}

#[test]
fn evidence_validation_voids_outstanding_approvals_when_content_is_unreadable() {
    let (mut h, ticket) = satisfied_at_tip_a();
    seat_and_observe(
        &mut h,
        ticket,
        snapshot(Some(&"a".repeat(40)), None, None),
        "/workspaces/kanban.unreadable",
        "unreadable",
    );
    let current = listed_binding(&h.core, ticket);
    assert_eq!(
        current["void"], true,
        "unreadable content must conservatively void outstanding approvals"
    );
    assert_eq!(current["satisfied"], false);
}

#[test]
fn evidence_validation_refuses_to_satisfy_from_an_invalidated_historical_approval() {
    let (mut h, ticket) = satisfied_at_tip_a();
    seat_and_observe(
        &mut h,
        ticket,
        snapshot(Some(&"b".repeat(40)), None, Some(true)),
        "/workspaces/kanban.new-head",
        "new-head",
    );
    let invalidated = listed_binding(&h.core, ticket);
    assert_eq!(invalidated["void"], true);
    assert_eq!(invalidated["satisfied"], false);
    let evidence = evidence_id(&h.core, ticket);
    let _ = h.core.command(
        "criterion.evidence.attach",
        &json!({
            "mutation": mutation(0, "rebind-old-tip-bind"),
            "ticket_id": ticket,
            "criterion_index": 0,
            "evidence_id": evidence,
            "tip": "a".repeat(40),
        }),
    );
    let _ = h.core.command(
        "criterion.evidence.review",
        &json!({
            "mutation": mutation(0, "rebind-old-tip-validate"),
            "ticket_id": ticket,
            "criterion_index": 0,
            "review": "validated",
        }),
    );
    let restored = satisfy(&h.core, ticket, &"a".repeat(40), "rebind-old-tip-satisfy");
    assert!(
        restored.is_err(),
        "an invalidated old tip must not be satisfied from its historical approved review"
    );
    let current = listed_binding(&h.core, ticket);
    assert_eq!(current["satisfied"], false);
}

fn attach_repository_at(core: &kanban_app::Core, ticket: u64, tip: &str, key: &str) -> u64 {
    core.command(
        "evidence.attach",
        &json!({
            "mutation": mutation(0, key),
            "project_id": 1,
            "entity_kind": "ticket",
            "entity_id": ticket.to_string(),
            "evidence_kind": "repository",
            "relative_path": "crates/kanban-app/src/tip_binding.rs",
            "commit_identity": tip,
        }),
    )
    .expect("evidence attaches")["id"]
        .as_u64()
        .expect("the evidence identity is a number")
}

fn submit_implementation_at(
    core: &kanban_app::Core,
    ticket: u64,
    tip: &str,
    key: &str,
) -> serde_json::Value {
    let queue = core
        .command(
            "dispatch.request",
            &json!({"mutation": mutation(0, format!("{key}-implement")), "ticket_id": ticket}),
        )
        .expect("a later implementation can be requested");
    let claim = core
        .command(
            "dispatch.claim",
            &json!({
                "mutation": mutation(1, format!("{key}-claim")),
                "dispatch_request_id": queue["id"],
            }),
        )
        .expect("the later implementation is claimed");
    let run = core
        .command(
            "run.acknowledge",
            &json!({
                "mutation": mutation(2, format!("{key}-run")),
                "dispatch_request_id": queue["id"],
            }),
        )
        .expect("the later implementation run starts");
    core.command(
        "submission.submit",
        &json!({
            "mutation": mutation(1, format!("{key}-result")),
            "run_id": run["id"],
            "capability_id": claim["capability"]["id"],
            "result": {
                "kind": "implementation",
                "tip": tip,
                "summary": "Second implementation tip",
            },
        }),
    )
    .expect("the later implementation submits")
}

fn approve_required_stage_at(
    core: &kanban_app::Core,
    review: &serde_json::Value,
    prefix: &str,
    tip: &str,
) -> serde_json::Value {
    let reviewer = &review["stages"][0]["slots"][0];
    let claim = core
        .command(
            "dispatch.claim",
            &json!({
                "mutation": mutation(1, format!("{prefix}-claim")),
                "dispatch_request_id": reviewer["dispatch_request_id"],
            }),
        )
        .expect("the required profile slot is claimed");
    let run = core
        .command(
            "run.acknowledge",
            &json!({
                "mutation": mutation(2, format!("{prefix}-run")),
                "dispatch_request_id": reviewer["dispatch_request_id"],
            }),
        )
        .expect("the required profile slot is acknowledged");
    core.command(
        "submission.submit",
        &json!({
            "mutation": mutation(1, format!("{prefix}-result")),
            "run_id": run["id"],
            "capability_id": claim["capability"]["id"],
            "result": {
                "kind": "review",
                "tip": tip,
                "summary": "Second tip verified",
                "approve": true,
            },
        }),
    )
    .expect("the required profile slot approves the later tip");
    let waiting = core
        .query("review.get", &json!({ "review_id": review["id"] }))
        .expect("the review is readable");
    core.command(
        "review.human.submit",
        &json!({
            "mutation": mutation(waiting["version"].as_u64().unwrap(), format!("{prefix}-human")),
            "review_id": review["id"],
            "slot_id": review["stages"][0]["slots"][1]["id"],
            "tip": tip,
            "approve": true,
            "summary": "Second human review passed",
        }),
    )
    .expect("the required human slot completes the later required stage")
}

#[test]
fn evidence_validation_refuses_only_the_a_approval_when_observing_approved_b() {
    let tip_a = "a".repeat(40);
    let tip_b = "b".repeat(40);
    let (mut h, ticket, submission_a) = bind_on_prepared(&tip_a);
    validate_bound_evidence(&h.core, ticket, "validate-a");
    let review_a = common::review::start(&h.core, ticket, &submission_a, "review-a");
    common::review::approve_required_stage(&h.core, &review_a, "review-a");
    satisfy(&h.core, ticket, &tip_a, "satisfy-a")
        .expect("a completed required-stage review at A satisfies");

    let submission_b = submit_implementation_at(&h.core, ticket, &tip_b, "b");
    let evidence_b = attach_repository_at(&h.core, ticket, &tip_b, "evidence-b");
    let review_b = common::review::start(&h.core, ticket, &submission_b, "review-b");
    approve_required_stage_at(&h.core, &review_b, "review-b", &tip_b);

    seat_and_observe(
        &mut h,
        ticket,
        snapshot(Some(&tip_b), Some(&"c".repeat(40)), Some(true)),
        "/workspaces/kanban.tip-b",
        "observe-b",
    );

    let invalidated = listed_binding(&h.core, ticket);
    assert_eq!(
        invalidated["void"], true,
        "observing B must void the remaining A binding"
    );
    assert_eq!(invalidated["satisfied"], false);

    let connection = rusqlite::Connection::open(&h.database_path).unwrap();
    let spent: Vec<(i64, String)> = connection
        .prepare(
            "SELECT review_id, tip FROM voided_approvals WHERE ticket_id=?1 ORDER BY review_id",
        )
        .unwrap()
        .query_map(rusqlite::params![ticket as i64], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .unwrap()
        .map(|row| row.unwrap())
        .collect();
    assert_eq!(
        spent,
        vec![(review_a["id"].as_i64().unwrap(), tip_a.clone())],
        "invalidation must spend only the approval that reviewed the A binding"
    );
    assert_ne!(
        review_a["id"], review_b["id"],
        "the later B review is a distinct approval"
    );

    let attached = h
        .core
        .command(
            "criterion.evidence.attach",
            &json!({
                "mutation": mutation(0, "bind-b"),
                "ticket_id": ticket,
                "criterion_index": 0,
                "evidence_id": evidence_b,
                "tip": tip_b,
            }),
        )
        .expect("evidence may attach at the live approved B tip");
    assert_eq!(attached["tip"], tip_b);
    assert_eq!(attached["void"], false);
    validate_bound_evidence(&h.core, ticket, "validate-b");
    let satisfied = satisfy(&h.core, ticket, &tip_b, "satisfy-b")
        .expect("AC2 can be satisfied at the current approved tip");
    assert_eq!(satisfied["satisfied"], true);
}
