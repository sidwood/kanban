mod common;

use common::{assign_lane, harness, insert_ticket, mutation};
use serde_json::json;

#[test]
fn submission_intake_records_an_authoritative_result_for_its_run() {
    let h = harness();
    let ticket = insert_ticket(&h.database_path, 1, "normal");
    assign_lane(&h.database_path, ticket);
    let queued = h
        .core
        .command(
            "dispatch.request",
            &json!({
                "mutation": mutation(0, "enqueue"), "ticket_id": ticket,
            }),
        )
        .unwrap();
    let claimed = h
        .core
        .command(
            "dispatch.claim",
            &json!({
                "mutation": mutation(1, "claim"), "dispatch_request_id": queued["id"],
            }),
        )
        .unwrap();
    let run = h
        .core
        .command(
            "run.acknowledge",
            &json!({
                "mutation": mutation(2, "acknowledge"), "dispatch_request_id": queued["id"],
            }),
        )
        .unwrap();
    let request = json!({
        "mutation": mutation(1, "submit"),
        "run_id": run["id"], "capability_id": claimed["capability"]["id"],
        "result": {"kind": "implementation", "tip": "a".repeat(40), "summary": "Verified the slice"},
    });
    let submitted = h
        .core
        .command("submission.submit", &request)
        .expect("a scoped structured result is accepted");
    assert_eq!(submitted["run_id"], run["id"]);
    assert_eq!(submitted["ticket_id"], ticket);
    assert_eq!(submitted["role"], "implementer");
    assert_eq!(submitted["result"], request["result"]);
    assert_eq!(
        h.core.command("submission.submit", &request).unwrap(),
        submitted
    );
    let conn = rusqlite::Connection::open(&h.database_path).unwrap();
    let rows: i64 = conn
        .query_row("SELECT COUNT(*) FROM submissions", [], |r| r.get(0))
        .unwrap();
    assert_eq!(rows, 1);
    let stored_id: i64 = conn
        .query_row(
            "SELECT json_extract(record, '$.id') FROM submissions",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(stored_id as u64, submitted["id"].as_u64().unwrap());

    let event: String = conn.query_row(
        "SELECT detail FROM timeline_events WHERE json_extract(detail, '$.action') = 'submission_received'",
        [], |r| r.get(0),
    ).unwrap();
    let event: serde_json::Value = serde_json::from_str(&event).unwrap();
    assert_eq!(event["run_id"], run["id"]);
    assert_eq!(event["role"], "implementer");
    let listed = h
        .core
        .query("submission.list", &json!({"project_id": 1}))
        .unwrap();
    assert_eq!(listed["submissions"], json!([submitted]));
}

fn ready_result() -> (common::DispatchHarness, serde_json::Value) {
    let h = harness();
    let ticket = insert_ticket(&h.database_path, 1, "normal");
    assign_lane(&h.database_path, ticket);
    let queued = h
        .core
        .command(
            "dispatch.request",
            &json!({
                "mutation": mutation(0, "queue"), "ticket_id": ticket,
            }),
        )
        .unwrap();
    let claimed = h
        .core
        .command(
            "dispatch.claim",
            &json!({
                "mutation": mutation(1, "claim"), "dispatch_request_id": queued["id"],
            }),
        )
        .unwrap();
    let run = h
        .core
        .command(
            "run.acknowledge",
            &json!({
                "mutation": mutation(2, "run"), "dispatch_request_id": queued["id"],
            }),
        )
        .unwrap();
    let request = json!({"mutation": mutation(1, "submit"), "run_id": run["id"],
        "capability_id": claimed["capability"]["id"],
        "result": {"kind": "implementation", "tip": "a".repeat(40), "summary": "Verified"}});
    (h, request)
}

#[test]
fn submission_intake_rejects_malformed_results_without_writes() {
    let invalid = [
        json!({"kind": "implementation", "tip": "main", "summary": "text"}),
        json!({"kind": "implementation", "tip": "a".repeat(40), "summary": "  "}),
        json!({"kind": "implementation", "tip": "z".repeat(40), "summary": "text"}),
        json!({"kind": "implementation", "tip": "a".repeat(40), "summary": "text", "approve": true}),
        json!({"kind": "review", "tip": "a".repeat(40), "summary": "text"}),
    ];
    for result in invalid {
        let (h, mut request) = ready_result();
        request["result"] = result;
        assert!(h.core.command("submission.submit", &request).is_err());
        assert_eq!(
            h.core
                .query("submission.list", &json!({"project_id": 1}))
                .unwrap()["submissions"],
            json!([])
        );
    }
}

#[test]
fn submission_intake_refuses_unknown_runs_and_inactive_or_wrong_scope_capabilities() {
    for change in [
        "unknown_run",
        "unknown_capability",
        "expired",
        "wrong_role",
        "no_grant",
        "wrong_ticket",
    ] {
        let (h, mut request) = ready_result();
        let conn = rusqlite::Connection::open(&h.database_path).unwrap();
        match change {
            "unknown_run" => request["run_id"] = json!(999),
            "unknown_capability" => request["capability_id"] = json!(999),
            "expired" => {
                conn.execute(
                    "UPDATE capabilities SET status = 'settled', settled_at = 1",
                    [],
                )
                .unwrap();
            }
            "wrong_role" => {
                conn.execute(
                    "UPDATE capabilities SET role = 'reviewer', reviewer_slot_id = 1",
                    [],
                )
                .unwrap();
            }
            "no_grant" => {
                conn.execute(
                    "UPDATE capabilities SET operations = '[\"ticket.get\"]'",
                    [],
                )
                .unwrap();
            }
            "wrong_ticket" => {
                let other = insert_ticket(&h.database_path, 2, "normal");
                conn.execute("UPDATE capabilities SET ticket_id = ?1", [other as i64])
                    .unwrap();
            }
            _ => unreachable!(),
        }
        assert!(
            h.core.command("submission.submit", &request).is_err(),
            "{change}"
        );
        let rows: i64 = conn
            .query_row("SELECT COUNT(*) FROM submissions", [], |r| r.get(0))
            .unwrap();
        assert_eq!(rows, 0, "{change}");
    }
}

#[test]
fn submission_intake_rolls_back_when_timeline_write_fails() {
    let (h, request) = ready_result();
    let conn = rusqlite::Connection::open(&h.database_path).unwrap();
    conn.execute_batch("CREATE TRIGGER reject_receipt BEFORE INSERT ON timeline_events WHEN json_extract(NEW.detail, '$.action') = 'submission_received' BEGIN SELECT RAISE(ABORT, 'fixture timeline failure'); END;").unwrap();
    assert!(h.core.command("submission.submit", &request).is_err());
    let rows: i64 = conn
        .query_row("SELECT COUNT(*) FROM submissions", [], |r| r.get(0))
        .unwrap();
    assert_eq!(rows, 0);
    conn.execute_batch("DROP TRIGGER reject_receipt;").unwrap();
    assert!(h.core.command("submission.submit", &request).is_ok());
}

#[test]
fn submission_intake_is_immutable_even_under_direct_sql_and_new_keys() {
    let (h, mut request) = ready_result();
    let received = h.core.command("submission.submit", &request).unwrap();
    request["mutation"] = mutation(1, "different-key");
    assert!(h.core.command("submission.submit", &request).is_err());
    let conn = rusqlite::Connection::open(&h.database_path).unwrap();
    for sql in [
        "UPDATE submissions SET record = '{}'",
        "DELETE FROM submissions",
        "INSERT OR REPLACE INTO submissions SELECT * FROM submissions",
    ] {
        assert!(conn.execute(sql, []).is_err(), "{sql}");
    }
    assert_eq!(
        h.core
            .query("submission.list", &json!({"project_id": 1}))
            .unwrap()["submissions"],
        json!([received])
    );
}

#[test]
fn submission_intake_retry_storm_has_one_result_and_one_receipt() {
    let (h, request) = ready_result();
    std::thread::scope(|scope| {
        let handles: Vec<_> = (0..8)
            .map(|_| scope.spawn(|| h.core.command("submission.submit", &request).unwrap()))
            .collect();
        let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        assert!(results.iter().all(|r| r == &results[0]));
    });
    let conn = rusqlite::Connection::open(&h.database_path).unwrap();
    let rows: i64 = conn.query_row("SELECT COUNT(*) FROM timeline_events WHERE json_extract(detail, '$.action') = 'submission_received'", [], |r| r.get(0)).unwrap();
    assert_eq!(rows, 1);
}

#[test]
fn missing_submission_checks_persisted_intake_and_project_correlation() {
    let (h, request) = ready_result();
    let database = kanban_storage::Database::open(&h.database_path).unwrap();
    let store = kanban_storage::SqliteSubmissionStore::new(&database);
    let event = json!({"kind": "role.settled", "run": request["run_id"]});
    assert!(
        kanban_app::submission::missing_submission_signal(&store, 1, &event)
            .unwrap()
            .is_some()
    );
    assert!(
        kanban_app::submission::missing_submission_signal(&store, 2, &event)
            .unwrap()
            .is_none()
    );
    h.core.command("submission.submit", &request).unwrap();
    assert!(
        kanban_app::submission::missing_submission_signal(&store, 1, &event)
            .unwrap()
            .is_none()
    );
    for event in [
        json!({"kind": "role.settled", "run": 999}),
        json!({"kind": "role.settled", "run": "nonsense"}),
        json!({"kind": "role.result", "run": 1}),
    ] {
        assert!(
            kanban_app::submission::missing_submission_signal(&store, 1, &event)
                .unwrap()
                .is_none()
        );
    }
}

#[test]
fn reviewer_submission_retains_the_exact_slot_and_verdict() {
    let (h, mut payload) = ready_result();
    rusqlite::Connection::open(&h.database_path).unwrap().execute(
        "UPDATE capabilities SET role='reviewer',reviewer_slot_id=7 WHERE dispatch_request_id=1", [],
    ).unwrap();
    payload["result"] = json!({"kind":"review","tip":"b".repeat(40),"summary":"Reviewed", "approve": true, "findings": []});
    let result = h
        .core
        .command("submission.submit", &payload)
        .expect("reviewer result accepted");
    assert_eq!(result["role"], "reviewer");
    assert_eq!(result["reviewer_slot_id"], 7);
    assert_eq!(result["result"], payload["result"]);
}
