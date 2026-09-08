mod common;
use common::mutation;
use common::review::prepared;
use serde_json::{Value, json};

fn valid_finding() -> Value {
    json!({"severity":"p1","in_scope":true,"summary":"Result custody is lost",
        "evidence":"The restart fixture reports a missing result","location":"recovery/result persistence",
        "proposed_resolution":"Persist the result before reporting success"})
}

#[test]
fn finding_rules_require_the_full_canonical_shape() {
    let valid = valid_finding();
    let finding: kanban_dto::ReviewFindingRecord =
        serde_json::from_value(valid.clone()).expect("the specified finding shape is accepted");
    assert_eq!(serde_json::to_value(finding).unwrap(), valid);
    for field in [
        "severity",
        "in_scope",
        "summary",
        "evidence",
        "location",
        "proposed_resolution",
    ] {
        let mut missing = valid.clone();
        missing.as_object_mut().unwrap().remove(field);
        assert!(
            serde_json::from_value::<kanban_dto::ReviewFindingRecord>(missing).is_err(),
            "{field} is required"
        );
    }
}

#[test]
fn finding_rules_refuse_blank_details_from_both_review_paths() {
    for agent in [false, true] {
        for field in ["summary", "evidence", "location", "proposed_resolution"] {
            let (h, ticket, submission) = prepared();
            let review=h.core.command("review.start",&json!({"mutation":mutation(0,"finding-review"),"ticket_id":ticket,"submission_id":submission["id"]})).unwrap();
            let mut finding = valid_finding();
            finding[field] = json!("  ");
            let reply = if agent {
                let slot = &review["stages"][0]["slots"][0];
                let claim=h.core.command("dispatch.claim",&json!({"mutation":mutation(1,"finding-claim"),"dispatch_request_id":slot["dispatch_request_id"]})).unwrap();
                let run=h.core.command("run.acknowledge",&json!({"mutation":mutation(2,"finding-run"),"dispatch_request_id":slot["dispatch_request_id"]})).unwrap();
                h.core.command("submission.submit",&json!({"mutation":mutation(1,"invalid-finding"),"run_id":run["id"],"capability_id":claim["capability"]["id"],
                    "result":{"kind":"review","tip":"a".repeat(40),"summary":"Review complete","approve":false,"findings":[finding]}}))
            } else {
                h.core.command("review.human.submit",&json!({"mutation":mutation(1,"invalid-finding"),"review_id":review["id"],"slot_id":review["stages"][0]["slots"][1]["id"],
                    "tip":"a".repeat(40),"summary":"Review complete","approve":false,"findings":[finding]}))
            };
            assert!(
                reply.is_err(),
                "blank {field} must be refused (agent={agent})"
            );
            assert_eq!(
                h.core
                    .query("review.get", &json!({"review_id":review["id"]}))
                    .unwrap(),
                review
            );
        }
    }
}

#[test]
fn finding_rules_blocking_requires_in_scope_p0_to_p2() {
    for (severity, in_scope, blocks) in [
        ("p0", true, true),
        ("p1", true, true),
        ("p2", true, true),
        ("p3", true, false),
        ("p0", false, false),
        ("p1", false, false),
        ("p2", false, false),
        ("p3", false, false),
    ] {
        let (h, ticket, submission) = prepared();
        let review=h.core.command("review.start",&json!({"mutation":mutation(0,"governance"),"ticket_id":ticket,"submission_id":submission["id"]})).unwrap();
        let mut finding = valid_finding();
        finding["severity"] = json!(severity);
        finding["in_scope"] = json!(in_scope);
        let payload = |approve, key: &str| {
            json!({"mutation":mutation(1,key),"review_id":review["id"],"slot_id":review["stages"][0]["slots"][1]["id"],
            "tip":"a".repeat(40),"summary":"Evidence reviewed","approve":approve,"findings":[finding]})
        };
        let invalid = h
            .core
            .command("review.human.submit", &payload(blocks, "inconsistent-vote"));
        assert!(
            invalid.is_err(),
            "contradictory verdict must be refused ({severity}, scope={in_scope})"
        );
        let recorded = h
            .core
            .command("review.human.submit", &payload(!blocks, "valid-vote"))
            .unwrap();
        assert_eq!(
            recorded["stages"][0]["slots"][1]["verdict"]["findings"][0],
            finding
        );
        let slot = &review["stages"][0]["slots"][0];
        let claim=h.core.command("dispatch.claim",&json!({"mutation":mutation(1,"governance-claim"),"dispatch_request_id":slot["dispatch_request_id"]})).unwrap();
        let run=h.core.command("run.acknowledge",&json!({"mutation":mutation(2,"governance-run"),"dispatch_request_id":slot["dispatch_request_id"]})).unwrap();
        h.core.command("submission.submit",&json!({"mutation":mutation(1,"governance-agent"),"run_id":run["id"],"capability_id":claim["capability"]["id"],
            "result":{"kind":"review","tip":"a".repeat(40),"summary":"No further findings","approve":true}})).unwrap();
        let resolved = h
            .core
            .query("review.get", &json!({"review_id":review["id"]}))
            .unwrap();
        assert_eq!(
            resolved["status"],
            if blocks { "rejected" } else { "approved" }
        );
    }
}
