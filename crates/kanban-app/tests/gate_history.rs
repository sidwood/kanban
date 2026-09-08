mod common;

use common::mutation;
use common::review::prepared;
use serde_json::json;

#[test]
fn gate_history_refuses_a_fresh_review_until_the_failed_gate_is_revalidated() {
    let (h, ticket, submission) = prepared();
    let review = h
        .core
        .command(
            "review.start",
            &json!({
                "mutation": mutation(0, "gate-review"),
                "ticket_id": ticket,
                "submission_id": submission["id"],
            }),
        )
        .unwrap();
    let slot = &review["stages"][0]["slots"][0];
    let claim = h
        .core
        .command(
            "dispatch.claim",
            &json!({
                "mutation": mutation(1, "gate-claim"),
                "dispatch_request_id": slot["dispatch_request_id"],
            }),
        )
        .unwrap();
    let run = h
        .core
        .command(
            "run.acknowledge",
            &json!({
                "mutation": mutation(2, "gate-run"),
                "dispatch_request_id": slot["dispatch_request_id"],
            }),
        )
        .unwrap();
    let defect = json!({
        "severity": "p1",
        "in_scope": true,
        "evidence": "The recovery fixture loses the accepted result",
        "summary": "Result custody is lost",
        "location": "recovery/result persistence",
        "proposed_resolution": "Persist before signalling success"
    });
    h.core
        .command(
            "submission.submit",
            &json!({
                "mutation": mutation(1, "gate-reject"),
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
        .unwrap();
    let waiting = h
        .core
        .query("review.get", &json!({"review_id": review["id"]}))
        .unwrap();
    h.core
        .command(
            "review.human.submit",
            &json!({
                "mutation": mutation(waiting["version"].as_u64().unwrap(), "gate-human"),
                "review_id": review["id"],
                "slot_id": review["stages"][0]["slots"][1]["id"],
                "tip": "a".repeat(40),
                "approve": true,
                "summary": "Human review complete"
            }),
        )
        .unwrap();
    let error = h
        .core
        .command(
            "review.start",
            &json!({
                "mutation": mutation(0, "gate-again"),
                "ticket_id": ticket,
                "submission_id": submission["id"],
            }),
        )
        .expect_err("a failed gate cannot start again without revalidation");
    assert_eq!(error.code, kanban_dto::ErrorCode::InvalidRequest);
    let history = h
        .core
        .query("review.history", &json!({"ticket_id": ticket}))
        .unwrap();
    assert_eq!(history["needs_revalidation"], true);
    assert_eq!(history["attempts"][0]["outcome"], "failed");
    assert!(
        history["attempts"][0]["verdicts"]
            .as_array()
            .unwrap()
            .iter()
            .any(|verdict| verdict == "rejected")
    );
    let revalidated = h
        .core
        .command(
            "review.revalidate",
            &json!({
                "mutation": mutation(0, "gate-revalidate"),
                "ticket_id": ticket,
            }),
        )
        .unwrap();
    assert_eq!(revalidated["needs_revalidation"], false);
    assert_eq!(revalidated["attempts"].as_array().unwrap().len(), 1);
    let restarted = h
        .core
        .command(
            "review.start",
            &json!({
                "mutation": mutation(0, "gate-restart"),
                "ticket_id": ticket,
                "submission_id": submission["id"],
            }),
        )
        .unwrap();
    assert_eq!(restarted["status"], "in_progress");
    let preserved = h
        .core
        .query("review.history", &json!({"ticket_id": ticket}))
        .unwrap();
    assert_eq!(preserved["attempts"][0]["outcome"], "failed");
}

#[test]
fn gate_history_refuses_approval_after_expiry_until_revalidation() {
    let (h, ticket, submission) = prepared();
    let review = h
        .core
        .command(
            "review.start",
            &json!({
                "mutation": mutation(0, "expire-review"),
                "ticket_id": ticket,
                "submission_id": submission["id"],
            }),
        )
        .unwrap();
    let expired = h
        .core
        .command(
            "review.expire",
            &json!({
                "mutation": mutation(review["version"].as_u64().unwrap(), "expire-gate"),
                "review_id": review["id"],
            }),
        )
        .unwrap();
    assert_eq!(expired["id"], review["id"]);
    let error = h
        .core
        .command(
            "review.start",
            &json!({
                "mutation": mutation(0, "expire-again"),
                "ticket_id": ticket,
                "submission_id": submission["id"],
            }),
        )
        .expect_err("an expired gate cannot start again without revalidation");
    assert_eq!(error.code, kanban_dto::ErrorCode::InvalidRequest);
    let history = h
        .core
        .query("review.history", &json!({"ticket_id": ticket}))
        .unwrap();
    assert_eq!(history["attempts"][0]["outcome"], "expired");
    h.core
        .command(
            "review.revalidate",
            &json!({
                "mutation": mutation(0, "expire-revalidate"),
                "ticket_id": ticket,
            }),
        )
        .unwrap();
    h.core
        .command(
            "review.start",
            &json!({
                "mutation": mutation(0, "expire-restart"),
                "ticket_id": ticket,
                "submission_id": submission["id"],
            }),
        )
        .unwrap();
}
