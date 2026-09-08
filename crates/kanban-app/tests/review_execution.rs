mod common;
use common::mutation;
use common::review::prepared;
use kanban_app::{ReviewConfigStore, TimelineEnvelope};
use kanban_domain::{ProfileName, ReviewSlot, ReviewStage, SlotRequirement, TicketId};
use kanban_dto::TimelineEventKind;
use serde_json::json;

#[test]
fn review_execution_stage_waits_for_required_slots_but_not_optional_slots() {
    let (h, ticket, submission) = prepared();
    let review = h
        .core
        .command(
            "review.start",
            &json!({"mutation":mutation(0,"review"),
        "ticket_id":ticket,"submission_id":submission["id"]}),
        )
        .expect("review starts from authoritative implementation evidence");
    assert_eq!(review["status"], "in_progress");
    let reviewer = &review["stages"][0]["slots"][0];
    let claim = h.core.command("dispatch.claim", &json!({"mutation":mutation(1,"review-claim"),"dispatch_request_id":reviewer["dispatch_request_id"]})).unwrap();
    assert_eq!(claim["capability"]["role"], "reviewer");
    assert_eq!(claim["capability"]["reviewer_slot_id"], reviewer["id"]);
    let run = h.core.command("run.acknowledge", &json!({"mutation":mutation(2,"review-run"),"dispatch_request_id":reviewer["dispatch_request_id"]})).unwrap();
    assert_eq!(run["requested"]["name"], "reviewer");
    assert_eq!(run["effective"]["name"], "reviewer");
    h.core
        .command(
            "submission.submit",
            &json!({"mutation":mutation(1,"review-result"),
                "run_id":run["id"],"capability_id":claim["capability"]["id"],
                "result":{"kind":"review","tip":"a".repeat(40),"summary":"Verified","approve":true}
            }),
        )
        .unwrap();
    let waiting = h
        .core
        .query("review.get", &json!({"review_id":review["id"]}))
        .unwrap();
    assert_eq!(waiting["status"], "in_progress");
    let resolved = h
        .core
        .command(
            "review.human.submit",
            &json!({"mutation":mutation(waiting["version"].as_u64().unwrap(),"human-review"),
        "review_id":review["id"],"slot_id":review["stages"][0]["slots"][1]["id"],
        "tip":"a".repeat(40),"approve":true,"summary":"Human review passed"}),
        )
        .unwrap();
    assert_eq!(resolved["status"], "approved");
    assert!(resolved["stages"][0]["slots"][2]["verdict"].is_null());
}

#[test]
fn review_execution_parallel_rejection_collects_one_bounce_after_all_required_slots() {
    let (h, ticket, submission) = prepared();
    let review=h.core.command("review.start",&json!({"mutation":mutation(0,"start-review"),"ticket_id":ticket,"submission_id":submission["id"]})).unwrap();
    let slot = &review["stages"][0]["slots"][0];
    let claim=h.core.command("dispatch.claim",&json!({"mutation":mutation(1,"claim-review"),"dispatch_request_id":slot["dispatch_request_id"]})).unwrap();
    let run=h.core.command("run.acknowledge",&json!({"mutation":mutation(2,"ack-review"),"dispatch_request_id":slot["dispatch_request_id"]})).unwrap();
    let defect = json!({"severity": "p1", "in_scope": true, "evidence": "The recovery fixture loses the accepted result", "summary": "Result custody is lost", "location": "recovery/result persistence", "proposed_resolution": "Persist before signalling success"});
    let result=h.core.command("submission.submit",&json!({"mutation":mutation(1,"reject-review"),"run_id":run["id"],
        "capability_id":claim["capability"]["id"],"result":{"kind":"review","tip":"a".repeat(40),"summary":"Fix required","approve":false,"findings":[defect]}})).expect("structured findings are retained");
    let waiting = h
        .core
        .query("review.get", &json!({"review_id":review["id"]}))
        .unwrap();
    assert_eq!(waiting["status"], "in_progress");
    assert!(waiting["bounce"].is_null());
    let improvement = json!({"severity": "p3", "in_scope": false, "evidence": "An optional explanatory label is missing", "summary": "An optional explanatory label is missing", "location": "review panel", "proposed_resolution": "Add an explanatory label"});
    let resolved=h.core.command("review.human.submit",&json!({"mutation":mutation(waiting["version"].as_u64().unwrap(),"human-finish"),
        "review_id":review["id"],"slot_id":review["stages"][0]["slots"][1]["id"],"tip":"a".repeat(40),"approve":true,
        "summary":"Human review complete","findings":[improvement]})).unwrap();
    assert_eq!(resolved["status"], "rejected");
    let findings = resolved["bounce"]["findings"].as_array().unwrap();
    assert_eq!(findings.len(), 2);
    assert_eq!(findings[0]["submission_id"], result["id"]);
    assert_eq!(findings[0]["finding"], defect);
    assert_eq!(findings[1]["finding"], improvement);
    let connection = rusqlite::Connection::open(&h.database_path).unwrap();
    let count:i64=connection.query_row("SELECT COUNT(*) FROM timeline_events WHERE json_extract(detail,'$.action')='review_stage_bounced'",[],|r|r.get(0)).unwrap();
    assert_eq!(count, 1);
}

#[test]
fn review_execution_ordered_stages_approve_only_the_same_tip() {
    let (h, ticket, submission) = prepared();
    let db = kanban_storage::Database::open(&h.database_path).unwrap();
    let store = kanban_storage::SqliteReviewConfigStore::new(&db);
    let mut configuration = store.find(TicketId::new(ticket)).unwrap().unwrap();
    configuration
        .replace(vec![
            ReviewStage::new(vec![ReviewSlot::profile(
                ProfileName::new("reviewer").unwrap(),
                SlotRequirement::Required,
            )]),
            ReviewStage::new(vec![ReviewSlot::human(SlotRequirement::Required)]),
        ])
        .unwrap();
    store
        .save(
            TicketId::new(ticket),
            &configuration,
            &TimelineEnvelope::project(
                1,
                TimelineEventKind::Transition,
                None,
                json!({"action":"fixture"}),
            ),
        )
        .unwrap();
    let review=h.core.command("review.start",&json!({"mutation":mutation(0,"ordered"),"ticket_id":ticket,"submission_id":submission["id"]})).unwrap();
    let human = review["stages"][1]["slots"][0]["id"].clone();
    let early=h.core.command("review.human.submit",&json!({"mutation":mutation(1,"early"),"review_id":review["id"],"slot_id":human,"tip":"a".repeat(40),"approve":true,"summary":"Too early"}));
    assert_eq!(
        early.unwrap_err().code,
        kanban_dto::ErrorCode::InvalidRequest
    );
    let slot = &review["stages"][0]["slots"][0];
    let claim=h.core.command("dispatch.claim",&json!({"mutation":mutation(1,"ordered-claim"),"dispatch_request_id":slot["dispatch_request_id"]})).unwrap();
    let run=h.core.command("run.acknowledge",&json!({"mutation":mutation(2,"ordered-run"),"dispatch_request_id":slot["dispatch_request_id"]})).unwrap();
    h.core.command("submission.submit",&json!({"mutation":mutation(1,"ordered-result"),"run_id":run["id"],"capability_id":claim["capability"]["id"],"result":{"kind":"review","tip":"a".repeat(40),"approve":true,"summary":"Reviewed"}})).unwrap();
    let waiting = h
        .core
        .query("review.get", &json!({"review_id":review["id"]}))
        .unwrap();
    assert_eq!(waiting["status"], "in_progress");
    assert_eq!(waiting["stages"][0]["status"], "approved");
    let version = waiting["version"].as_u64().unwrap();
    let wrong=h.core.command("review.human.submit",&json!({"mutation":mutation(version,"wrong-tip"),"review_id":review["id"],"slot_id":human,"tip":"b".repeat(40),"approve":true,"summary":"Different code"}));
    assert_eq!(
        wrong.unwrap_err().code,
        kanban_dto::ErrorCode::InvalidRequest
    );
    assert_eq!(
        h.core
            .query("review.get", &json!({"review_id":review["id"]}))
            .unwrap(),
        waiting
    );
    let complete=h.core.command("review.human.submit",&json!({"mutation":mutation(version,"right-tip"),"review_id":review["id"],"slot_id":human,"tip":"a".repeat(40),"approve":true,"summary":"Same code verified"})).unwrap();
    assert_eq!(complete["status"], "approved");
    assert!(
        complete["stages"]
            .as_array()
            .unwrap()
            .iter()
            .all(|stage| stage["status"] == "approved")
    );
}

#[test]
fn review_execution_wakes_the_coordinator_for_new_reviewer_requests_once() {
    let (h, ticket, submission) = prepared();
    h.wake.calls.lock().unwrap().clear();
    let request = json!({"mutation":mutation(0,"wake-review"),"ticket_id":ticket,"submission_id":submission["id"]});
    let started = h.core.command("review.start", &request).unwrap();
    let calls = h.wake.calls.lock().unwrap().clone();
    assert_eq!(
        calls.len(),
        1,
        "a queued review must wake its Coordinator after commit"
    );
    assert_eq!(calls[0].project_id, 1);
    assert_eq!(
        json!(calls[0].dispatch_request_id),
        started["stages"][0]["slots"][0]["dispatch_request_id"]
    );
    assert_eq!(h.core.command("review.start", &request).unwrap(), started);
    assert_eq!(
        h.wake.calls.lock().unwrap().len(),
        1,
        "replay never wakes twice"
    );
}

#[test]
fn review_execution_wakes_the_next_stage_from_agent_or_human_verdicts() {
    for agent_first in [true, false] {
        let (h, ticket, submission) = prepared();
        let db = kanban_storage::Database::open(&h.database_path).unwrap();
        let store = kanban_storage::SqliteReviewConfigStore::new(&db);
        let mut configuration = store.find(TicketId::new(ticket)).unwrap().unwrap();
        let first = if agent_first {
            ReviewSlot::profile(
                ProfileName::new("reviewer").unwrap(),
                SlotRequirement::Required,
            )
        } else {
            ReviewSlot::human(SlotRequirement::Required)
        };
        configuration
            .replace(vec![
                ReviewStage::new(vec![first]),
                ReviewStage::new(vec![ReviewSlot::profile(
                    ProfileName::new("reviewer").unwrap(),
                    SlotRequirement::Required,
                )]),
            ])
            .unwrap();
        store
            .save(
                TicketId::new(ticket),
                &configuration,
                &TimelineEnvelope::project(
                    1,
                    TimelineEventKind::Transition,
                    None,
                    json!({"action":"fixture"}),
                ),
            )
            .unwrap();
        let review=h.core.command("review.start",&json!({"mutation":mutation(0,"advance-review"),"ticket_id":ticket,"submission_id":submission["id"]})).unwrap();
        assert!(review["stages"][1]["slots"][0]["dispatch_request_id"].is_null());
        h.wake.calls.lock().unwrap().clear();
        let slot = &review["stages"][0]["slots"][0];
        if agent_first {
            let claim=h.core.command("dispatch.claim",&json!({"mutation":mutation(1,"advance-claim"),"dispatch_request_id":slot["dispatch_request_id"]})).unwrap();
            let run=h.core.command("run.acknowledge",&json!({"mutation":mutation(2,"advance-run"),"dispatch_request_id":slot["dispatch_request_id"]})).unwrap();
            h.core.command("submission.submit",&json!({"mutation":mutation(1,"advance-result"),"run_id":run["id"],"capability_id":claim["capability"]["id"],"result":{"kind":"review","tip":"a".repeat(40),"summary":"Verified","approve":true}})).unwrap();
        } else {
            h.core.command("review.human.submit",&json!({"mutation":mutation(1,"advance-human"),"review_id":review["id"],"slot_id":slot["id"],"tip":"a".repeat(40),"approve":true,"summary":"Verified"})).unwrap();
        }
        let advanced = h
            .core
            .query("review.get", &json!({"review_id":review["id"]}))
            .unwrap();
        let calls = h.wake.calls.lock().unwrap();
        assert_eq!(
            calls.len(),
            1,
            "a completed required stage must wake its successor"
        );
        assert_eq!(
            json!(calls[0].dispatch_request_id),
            advanced["stages"][1]["slots"][0]["dispatch_request_id"]
        );
    }
}

#[test]
fn review_execution_dispatch_exposes_frozen_reviewer_instructions() {
    let (h, ticket, submission) = prepared();
    let review=h.core.command("review.start",&json!({"mutation":mutation(0,"instructions"),"ticket_id":ticket,"submission_id":submission["id"]})).unwrap();
    let queue = h
        .core
        .query("dispatch.queue", &json!({"project_id":1}))
        .unwrap();
    let request = queue["requests"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["id"] == review["stages"][0]["slots"][0]["dispatch_request_id"])
        .unwrap();
    let instructions = &request["reviewer"];
    assert_eq!(instructions["review_id"], review["id"]);
    assert_eq!(
        instructions["slot_id"],
        review["stages"][0]["slots"][0]["id"]
    );
    assert_eq!(instructions["tip"], submission["result"]["tip"]);
    assert_eq!(instructions["effective"]["name"], "reviewer");
    assert_eq!(instructions["effective"]["harness"], "codex-cli");
    let claim=h.core.command("dispatch.claim",&json!({"mutation":mutation(1,"instruction-claim"),"dispatch_request_id":request["id"]})).unwrap();
    assert_eq!(claim["request"]["reviewer"], *instructions);
}

#[test]
fn review_execution_late_optional_verdict_is_retained_without_reopening_resolution() {
    let (h, ticket, submission) = prepared();
    let review=h.core.command("review.start",&json!({"mutation":mutation(0,"late-optional"),"ticket_id":ticket,"submission_id":submission["id"]})).unwrap();
    let slot = &review["stages"][0]["slots"][0];
    let claim=h.core.command("dispatch.claim",&json!({"mutation":mutation(1,"late-claim"),"dispatch_request_id":slot["dispatch_request_id"]})).unwrap();
    let run=h.core.command("run.acknowledge",&json!({"mutation":mutation(2,"late-run"),"dispatch_request_id":slot["dispatch_request_id"]})).unwrap();
    h.core.command("submission.submit",&json!({"mutation":mutation(1,"late-required-agent"),"run_id":run["id"],"capability_id":claim["capability"]["id"],"result":{"kind":"review","tip":"a".repeat(40),"summary":"Verified","approve":true}})).unwrap();
    let waiting = h
        .core
        .query("review.get", &json!({"review_id":review["id"]}))
        .unwrap();
    let approved=h.core.command("review.human.submit",&json!({"mutation":mutation(waiting["version"].as_u64().unwrap(),"late-required-human"),"review_id":review["id"],"slot_id":review["stages"][0]["slots"][1]["id"],"tip":"a".repeat(40),"approve":true,"summary":"Verified"})).unwrap();
    assert_eq!(approved["status"], "approved");
    let late=h.core.command("review.human.submit",&json!({"mutation":mutation(approved["version"].as_u64().unwrap(),"late-advisory"),"review_id":review["id"],"slot_id":review["stages"][0]["slots"][2]["id"],"tip":"a".repeat(40),"approve":false,"summary":"Advisory follow-up"})).expect("late optional work remains auditable");
    assert_eq!(late["status"], "approved");
    assert_eq!(late["stages"][0]["status"], "approved");
    assert_eq!(
        late["stages"][0]["slots"][2]["verdict"]["summary"],
        "Advisory follow-up"
    );
    assert_eq!(
        late["stages"][0]["slots"][2]["verdict"]["counts_for_resolution"],
        false
    );
    assert!(late["bounce"].is_null());
}
