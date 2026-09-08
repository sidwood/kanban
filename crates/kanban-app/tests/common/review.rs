//! Reusable review fixtures through the real SQLite Core.

use super::{assign_lane, harness, insert_ticket, mutation};
use kanban_app::{ProfileStore, ReviewConfigStore, TimelineEnvelope};
use kanban_domain::{
    ExecutionProfile, ProfileDefinition, ProfileName, ProjectId, ReviewConfiguration, ReviewSlot,
    ReviewStage, SlotRequirement, TicketId,
};
use kanban_dto::TimelineEventKind;
use serde_json::{Value, json};

pub fn prepared() -> (super::DispatchHarness, u64, Value) {
    let h = harness();
    let ticket = insert_ticket(&h.database_path, 1, "normal");
    assign_lane(&h.database_path, ticket);
    rusqlite::Connection::open(&h.database_path).unwrap().execute(
        "UPDATE projects SET ticket_counter=(SELECT COALESCE(MAX(number),0) FROM tickets WHERE project_id=projects.id) WHERE id=1",[],
    ).unwrap();
    let queue = h
        .core
        .command(
            "dispatch.request",
            &json!({"mutation":mutation(0,"implement"),"ticket_id":ticket}),
        )
        .unwrap();
    let claim = h
        .core
        .command(
            "dispatch.claim",
            &json!({"mutation":mutation(1,"claim"),"dispatch_request_id":queue["id"]}),
        )
        .unwrap();
    let run = h
        .core
        .command(
            "run.acknowledge",
            &json!({"mutation":mutation(2,"run"),"dispatch_request_id":queue["id"]}),
        )
        .unwrap();
    let submission = h.core.command("submission.submit", &json!({
        "mutation":mutation(1,"result"),"run_id":run["id"],"capability_id":claim["capability"]["id"],
        "result":{"kind":"implementation","tip":"a".repeat(40),"summary":"Implemented and verified"}
    })).unwrap();
    let db = kanban_storage::Database::open(&h.database_path).unwrap();
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
                ProjectId::new(1).value(),
                TimelineEventKind::Transition,
                None,
                json!({"action":"fixture"}),
            ),
        )
        .unwrap();
    (h, ticket, submission)
}
