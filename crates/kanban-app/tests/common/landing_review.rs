//! Ticket review and criterion sequence required before ordinary
//! landing (KAN-T52 G5/G6, KAN-S10-US7).

use std::path::Path;
use std::process::Command;
use std::sync::Arc;

use kanban_app::{Core, NoopCoordinatorWake};
use kanban_storage::Database;
use serde_json::{Value, json};

use super::mutation;

/// Register the operations the source-tip review sequence needs on a
/// landing fixture Core that already has Tickets, Lanes, and landing
/// commands.
pub fn register_source_review(core: &mut Core, database: &Database, scratch: &Path) {
    let attachments = scratch.join("attachments");
    std::fs::create_dir_all(&attachments).expect("the attachment directory exists");
    let projects = Arc::new(kanban_storage::SqliteProjectStore::new(database));
    let tickets = Arc::new(kanban_storage::SqliteTicketStore::new(database));
    let specs = Arc::new(kanban_storage::SqliteSpecStore::new(database));
    let profiles = Arc::new(kanban_storage::SqliteProfileStore::new(database));
    let dependencies = Arc::new(kanban_storage::SqliteDependencyStore::new(database));
    let proposals = Arc::new(kanban_storage::SqliteGraphProposalStore::new(database));
    let dispatch = Arc::new(kanban_storage::SqliteDispatchStore::new(database));
    let wake = Arc::new(NoopCoordinatorWake);
    let evidence = Arc::new(kanban_storage::SqliteEvidenceStore::new(
        database,
        attachments,
    ));
    core.register_lifecycle(
        tickets.clone(),
        dependencies.clone(),
        projects.clone(),
        Arc::new(kanban_storage::SqliteScheduleStore::new(database)),
        Some(Arc::new(kanban_storage::SqliteCriterionBindingStore::new(
            database,
        ))),
    )
    .expect("the lifecycle operations register");
    core.register_profiles(profiles.clone(), tickets.clone(), projects.clone())
        .expect("the profile operations register");
    core.register_dependencies(dependencies.clone(), tickets.clone(), projects.clone())
        .expect("the dependency operations register");
    core.register_graph_proposals(
        proposals.clone(),
        dependencies.clone(),
        tickets.clone(),
        specs.clone(),
        projects.clone(),
        profiles.clone(),
    )
    .expect("the graph operations register");
    core.register_dispatch(
        dispatch.clone(),
        tickets.clone(),
        profiles.clone(),
        projects.clone(),
        Arc::new(kanban_storage::SqliteCapacityStore::new(database)),
        Arc::new(kanban_storage::SqliteLaneStore::new(database)),
        dependencies.clone(),
        proposals.clone(),
        wake.clone(),
    )
    .expect("the dispatch operations register");
    core.register_runs(
        Arc::new(kanban_storage::SqliteRunStore::new(database)),
        dispatch,
        tickets.clone(),
        profiles.clone(),
        projects.clone(),
        dependencies,
        proposals,
    )
    .expect("the run operations register");
    core.register_submissions(
        Arc::new(kanban_storage::SqliteSubmissionStore::new(database)),
        Arc::new(kanban_storage::SqliteCapabilityStore::new(database)),
        projects.clone(),
        wake.clone(),
    )
    .expect("the submission operations register");
    core.register_reviews(
        Arc::new(kanban_storage::SqliteReviewExecutionStore::new(database)),
        Arc::new(kanban_storage::SqliteReviewConfigStore::new(database)),
        tickets.clone(),
        profiles.clone(),
        projects.clone(),
        Arc::new(kanban_storage::SqliteSubmissionStore::new(database)),
        Arc::new(kanban_storage::SqliteRunStore::new(database)),
        wake,
    )
    .expect("the review operations register");
    core.register_review_config(
        Arc::new(kanban_storage::SqliteReviewConfigStore::new(database)),
        tickets.clone(),
        profiles,
        projects.clone(),
    )
    .expect("the review configuration operations register");
    core.register_evidence(evidence.clone(), projects)
        .expect("the evidence operations register");
    core.register_criterion_bindings(
        Arc::new(kanban_storage::SqliteCriterionBindingStore::new(database)),
        tickets,
        evidence,
        Arc::new(kanban_storage::SqliteReviewExecutionStore::new(database)),
    )
    .expect("the criterion binding operations register");
}

/// Define the implementer and harness-separated reviewer profiles.
pub fn seed_review_profiles(core: &Core) {
    core.command(
        "profile.define",
        &json!({
            "mutation": mutation(0, "profile-standard"),
            "name": "standard",
            "harness": "claude-code",
            "model": "opus",
            "effort": "high",
            "usage_pool": "operator",
        }),
    )
    .expect("the implementer profile defines");
    core.command(
        "profile.define",
        &json!({
            "mutation": mutation(0, "profile-reviewer"),
            "name": "reviewer",
            "harness": "codex-cli",
            "model": "gpt",
            "effort": "high",
            "usage_pool": "operator",
        }),
    )
    .expect("the reviewer profile defines");
}

/// Run the Ticket review sequence at the source Workspace tip, and
/// optionally satisfy every criterion at that tip.
pub fn complete_source_review(
    core: &Core,
    ticket: &Value,
    spec: Option<&Value>,
    source: &Path,
    relative_path: &str,
    key: &str,
    satisfy_criteria: bool,
) {
    let tip = head_tip(source);
    let ticket_id = ticket["id"]
        .as_u64()
        .expect("the Ticket identity is a number");
    let mut current = ticket.clone();
    if ticket["kind"] == "bug" && ticket.get("qualification").is_none_or(Value::is_null) {
        current = core
            .command(
                "ticket.bug.qualify",
                &json!({
                    "mutation": mutation(version(&current), format!("{key}-qualify")),
                    "ticket_id": ticket_id,
                    "qualification": {
                        "expected_behaviour": "The standalone Bug lands only after review.",
                        "reproduction": "Land without a Ticket review.",
                        "environment": "macOS, disposable git fixtures.",
                        "severity": "high",
                        "frequency": "Every unreviewed landing.",
                        "affected_scope": "Seed landing.",
                        "risk": "Unreviewed code lands.",
                        "criteria": [{
                            "outcome": "The source tip is reviewed before the merge.",
                            "stories": ["CORE-S1-US7"]
                        }],
                        "verification_steps": [{ "command": "cargo test -p kanban-app --test standalone_bug_landing" }]
                    }
                }),
            )
            .expect("the Bug qualifies");
    }
    let parked = core
        .command(
            "ticket.park",
            &json!({
                "mutation": mutation(version(&current), format!("{key}-park")),
                "ticket_id": ticket_id,
            }),
        )
        .expect("the Ticket parks");
    let ready = core
        .command(
            "ticket.unpark",
            &json!({
                "mutation": mutation(version(&parked), format!("{key}-unpark")),
                "ticket_id": ticket_id,
            }),
        )
        .expect("the Ticket returns to ready");
    core.command(
        "ticket.assign",
        &json!({
            "mutation": mutation(version(&ready), format!("{key}-assign")),
            "ticket_id": ticket_id,
            "profile": "standard",
        }),
    )
    .expect("the implementer profile assigns");
    if let Some(spec) = spec {
        let standing = core
            .query("spec.get", &json!({ "spec_id": spec["id"] }))
            .expect("the Spec reads");
        if standing["versions"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|version| version["state"] == "draft")
        {
            core.command(
                "spec.version.approve",
                &json!({
                    "mutation": mutation(
                        standing["spec"]["version"]
                            .as_u64()
                            .expect("the Spec version is a number"),
                        format!("{key}-spec-approve"),
                    ),
                    "spec_id": spec["id"],
                }),
            )
            .expect("the Spec content version approves");
        }
        let proposal = core
            .command(
                "ticket.graph.propose",
                &json!({
                    "mutation": mutation(0, format!("{key}-graph-propose")),
                    "spec_id": spec["id"],
                    "spec_version": 1,
                    "tickets": [ticket_id],
                    "edges": [],
                }),
            )
            .expect("the Ticket graph proposes");
        core.command(
            "ticket.graph.approve",
            &json!({
                "mutation": mutation(version(&proposal), format!("{key}-graph-approve")),
                "proposal_id": proposal["id"],
            }),
        )
        .expect("the Ticket graph approves");
    }
    let queue = core
        .command(
            "dispatch.request",
            &json!({
                "mutation": mutation(0, format!("{key}-dispatch")),
                "ticket_id": ticket_id,
            }),
        )
        .expect("the implementer run is requested");
    let claim = core
        .command(
            "dispatch.claim",
            &json!({
                "mutation": mutation(1, format!("{key}-claim")),
                "dispatch_request_id": queue["id"],
            }),
        )
        .expect("the implementer run is claimed");
    let run = core
        .command(
            "run.acknowledge",
            &json!({
                "mutation": mutation(2, format!("{key}-run")),
                "dispatch_request_id": queue["id"],
            }),
        )
        .expect("the implementer run acknowledges");
    let submission = core
        .command(
            "submission.submit",
            &json!({
                "mutation": mutation(1, format!("{key}-implemented")),
                "run_id": run["id"],
                "capability_id": claim["capability"]["id"],
                "result": {
                    "kind": "implementation",
                    "tip": tip,
                    "summary": "Implemented and verified at the source tip"
                }
            }),
        )
        .expect("the implementer submits the source tip");
    core.command(
        "ticket.review.configure",
        &json!({
            "mutation": mutation(0, format!("{key}-configure")),
            "ticket_id": ticket_id,
            "stages": [{
                "slots": [
                    {
                        "occupant": { "kind": "profile", "name": "reviewer" },
                        "requirement": "optional"
                    },
                    {
                        "occupant": { "kind": "human" },
                        "requirement": "required"
                    }
                ]
            }],
        }),
    )
    .expect("the Ticket review stages configure");
    let review = core
        .command(
            "review.start",
            &json!({
                "mutation": mutation(0, format!("{key}-review-start")),
                "ticket_id": ticket_id,
                "submission_id": submission["id"],
            }),
        )
        .expect("the Ticket review starts at the source tip");
    let human = review["stages"][0]["slots"]
        .as_array()
        .expect("the first stage has slots")
        .iter()
        .find(|slot| slot["occupant"]["kind"] == "human")
        .expect("a required human slot is configured");
    core.command(
        "review.human.submit",
        &json!({
            "mutation": mutation(
                review["version"].as_u64().expect("the review version is a number"),
                format!("{key}-review-human"),
            ),
            "review_id": review["id"],
            "slot_id": human["id"],
            "tip": tip,
            "approve": true,
            "summary": "The source tip is accepted for landing",
        }),
    )
    .expect("the required human slot approves the source tip");
    if !satisfy_criteria {
        return;
    }
    let criteria = source_review_criteria(&current);
    if criteria == 0 {
        return;
    }
    let evidence = core
        .command(
            "evidence.attach",
            &json!({
                "mutation": mutation(0, format!("{key}-evidence")),
                "project_id": 1,
                "entity_kind": "ticket",
                "entity_id": ticket_id.to_string(),
                "evidence_kind": "repository",
                "relative_path": relative_path,
                "commit_identity": tip,
            }),
        )
        .expect("implementer evidence attaches");
    for index in 0..criteria {
        core.command(
            "criterion.evidence.attach",
            &json!({
                "mutation": mutation(0, format!("{key}-bind-{index}")),
                "ticket_id": ticket_id,
                "criterion_index": index,
                "evidence_id": evidence["id"],
                "tip": tip,
            }),
        )
        .expect("evidence binds to the criterion at the source tip");
        core.command(
            "criterion.evidence.review",
            &json!({
                "mutation": mutation(0, format!("{key}-validate-{index}")),
                "ticket_id": ticket_id,
                "criterion_index": index,
                "review": "validated",
            }),
        )
        .expect("reviewers validate the bound evidence");
        core.command(
            "criterion.satisfy",
            &json!({
                "mutation": mutation(0, format!("{key}-satisfy-{index}")),
                "ticket_id": ticket_id,
                "criterion_index": index,
                "tip": tip,
            }),
        )
        .expect("the criterion is satisfied at the source tip");
    }
}

fn source_review_criteria(ticket: &Value) -> usize {
    if ticket["kind"] == "bug" {
        ticket
            .get("bug")
            .and_then(|body| body.get("qualification"))
            .and_then(|qualification| qualification.get("criteria"))
            .and_then(Value::as_array)
            .map_or(0, Vec::len)
    } else {
        ticket
            .get("criteria")
            .and_then(Value::as_array)
            .map_or(0, Vec::len)
    }
}

fn version(record: &Value) -> u64 {
    record["version"]
        .as_u64()
        .expect("the record carries a version")
}

fn head_tip(dir: &Path) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["rev-parse", "HEAD"])
        .output()
        .expect("git rev-parse runs");
    assert!(output.status.success(), "git rev-parse HEAD succeeds");
    String::from_utf8(output.stdout)
        .expect("the tip is UTF-8")
        .trim()
        .to_owned()
}
