//! Reusable review fixtures through the real SQLite Core.

use super::{assign_lane_in, harness, insert_ready_ticket_in, mutation};
use kanban_app::{ProfileStore, ReviewConfigStore, TimelineEnvelope};
use kanban_domain::{
    ExecutionProfile, ProfileDefinition, ProfileName, ProjectId, ReviewConfiguration, ReviewSlot,
    ReviewStage, SlotRequirement, TicketId,
};
use kanban_dto::TimelineEventKind;
use serde_json::{Value, json};

pub fn prepared() -> (super::DispatchHarness, u64, Value) {
    let h = harness();
    let (ticket, submission) = prepare_on(&h.core, &h.database_path);
    (h, ticket, submission)
}

pub fn prepare_on(core: &kanban_app::Core, database_path: &std::path::Path) -> (u64, Value) {
    prepare_on_in(core, database_path, 1, 1)
}

/// Seat a ready Task in `project_id` with an implementation submission
/// and a required-stage review configuration, so a test can complete
/// a ReviewExecution before `criterion.satisfy`.
pub fn prepare_on_in(
    core: &kanban_app::Core,
    database_path: &std::path::Path,
    project_id: u64,
    number: u64,
) -> (u64, Value) {
    let ticket = insert_ready_ticket_in(database_path, project_id, number, "normal");
    assign_lane_in(database_path, project_id, ticket);
    rusqlite::Connection::open(database_path)
        .unwrap()
        .execute(
            "UPDATE projects SET ticket_counter=(SELECT COALESCE(MAX(number),0) FROM tickets WHERE project_id=projects.id) WHERE id=?1",
            rusqlite::params![project_id as i64],
        )
        .unwrap();
    let queue = core
        .command(
            "dispatch.request",
            &json!({"mutation":mutation(0,"implement"),"ticket_id":ticket}),
        )
        .unwrap();
    let claim = core
        .command(
            "dispatch.claim",
            &json!({"mutation":mutation(1,"claim"),"dispatch_request_id":queue["id"]}),
        )
        .unwrap();
    let run = core
        .command(
            "run.acknowledge",
            &json!({"mutation":mutation(2,"run"),"dispatch_request_id":queue["id"]}),
        )
        .unwrap();
    let submission = core.command("submission.submit", &json!({
        "mutation":mutation(1,"result"),"run_id":run["id"],"capability_id":claim["capability"]["id"],
        "result":{"kind":"implementation","tip":"a".repeat(40),"summary":"Implemented and verified"}
    })).unwrap();
    let db = kanban_storage::Database::open(database_path).unwrap();
    let profile = ExecutionProfile::define(
        ProfileName::new("reviewer").unwrap(),
        ProfileDefinition::new("codex-cli", "gpt", "high", "operator", None).unwrap(),
    )
    .unwrap();
    kanban_storage::SqliteProfileStore::new(&db)
        .define(
            &profile,
            &TimelineEnvelope::global(
                TimelineEventKind::Transition,
                None,
                json!({"action":"fixture"}),
            ),
        )
        .unwrap();
    let configuration = ReviewConfiguration::new(vec![ReviewStage::new(vec![
        ReviewSlot::profile(
            ProfileName::new("reviewer").unwrap(),
            SlotRequirement::Required,
        ),
        ReviewSlot::human(SlotRequirement::Required),
        ReviewSlot::human(SlotRequirement::Optional),
    ])])
    .unwrap();
    kanban_storage::SqliteReviewConfigStore::new(&db)
        .insert(
            TicketId::new(ticket),
            &configuration,
            &TimelineEnvelope::project(
                ProjectId::new(project_id).value(),
                TimelineEventKind::Transition,
                None,
                json!({"action":"fixture"}),
            ),
        )
        .unwrap();
    (ticket, submission)
}

pub fn start(core: &kanban_app::Core, ticket: u64, submission: &Value, key: &str) -> Value {
    core.command(
        "review.start",
        &json!({
            "mutation": mutation(0, key),
            "ticket_id": ticket,
            "submission_id": submission["id"],
        }),
    )
    .expect("review starts from the implementer's result")
}

pub fn approve_required_stage(core: &kanban_app::Core, review: &Value, prefix: &str) -> Value {
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
                "tip": "a".repeat(40),
                "summary": "Verified",
                "approve": true
            }
        }),
    )
    .expect("the required profile slot approves the implementation tip");
    let waiting = core
        .query("review.get", &json!({ "review_id": review["id"] }))
        .expect("the review is readable");
    core.command(
        "review.human.submit",
        &json!({
            "mutation": mutation(waiting["version"].as_u64().unwrap(), format!("{prefix}-human")),
            "review_id": review["id"],
            "slot_id": review["stages"][0]["slots"][1]["id"],
            "tip": "a".repeat(40),
            "approve": true,
            "summary": "Human review passed",
        }),
    )
    .expect("the required human slot completes the required stage")
}

pub fn reject_required_stage(core: &kanban_app::Core, review: &Value, prefix: &str) -> Value {
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
    let defect = json!({
        "severity": "p1",
        "in_scope": true,
        "evidence": "The recovery fixture loses the accepted result",
        "summary": "Result custody is lost",
        "location": "recovery/result persistence",
        "proposed_resolution": "Persist before signalling success"
    });
    core.command(
        "submission.submit",
        &json!({
            "mutation": mutation(1, format!("{prefix}-result")),
            "run_id": run["id"],
            "capability_id": claim["capability"]["id"],
            "result": {
                "kind": "review",
                "tip": "a".repeat(40),
                "summary": "Fix required",
                "approve": false,
                "findings": [defect]
            }
        }),
    )
    .expect("the required profile slot rejects the implementation tip");
    let waiting = core
        .query("review.get", &json!({ "review_id": review["id"] }))
        .expect("the review is readable");
    core.command(
        "review.human.submit",
        &json!({
            "mutation": mutation(waiting["version"].as_u64().unwrap(), format!("{prefix}-human")),
            "review_id": review["id"],
            "slot_id": review["stages"][0]["slots"][1]["id"],
            "tip": "a".repeat(40),
            "approve": true,
            "summary": "Human review complete",
            "findings": [{
                "severity": "p3",
                "in_scope": false,
                "evidence": "An optional explanatory label is missing",
                "summary": "An optional explanatory label is missing",
                "location": "review panel",
                "proposed_resolution": "Add an explanatory label"
            }]
        }),
    )
    .expect("the parallel required human slot finishes so the stage can bounce")
}
