mod common;
use common::{assign_lane, harness, insert_ticket, mutation};
use kanban_app::ProjectStore;
use kanban_domain::ProjectId;
use kanban_herdr::fixture::{ScriptedSession, SessionScript};
use kanban_service::herdr::{BackoffPolicy, HerdrObserver, ObservationTuning};
use kanban_storage::{Database, SqliteProjectStore};
use serde_json::json;
use std::sync::Arc;
use std::time::{Duration, Instant};

#[test]
fn missing_submission_from_settled_agent_is_not_silenced_by_result_telemetry() {
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
    h.core
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
    let socket_root = h._dir.path().join("sessions");
    let _fixture = ScriptedSession::bind(&socket_root, "kanban-main", "/workspaces/kanban.seed",
        SessionScript::default().with_events(vec![
            json!({"kind": "role.settled", "role": "implementer", "run": run["id"]}),
            json!({"kind": "role.result", "role": "implementer", "run": run["id"], "approve": true}),
        ]));
    let database = Arc::new(Database::open(&h.database_path).unwrap());
    let project = SqliteProjectStore::new(&database)
        .find(ProjectId::new(1))
        .unwrap()
        .unwrap();
    let observer = HerdrObserver::with_observation(
        database,
        socket_root,
        ObservationTuning {
            backoff: BackoffPolicy::new(Duration::from_millis(10), Duration::from_millis(50)),
            settle: Duration::from_millis(10),
            io_timeout: Duration::from_millis(100),
        },
    );
    observer.observe_projects(&[project]);
    let until = Instant::now() + Duration::from_secs(2);
    let signal = loop {
        if let Some(signal) = observer
            .attention_signals(1)
            .into_iter()
            .find(|s| s.reason == "missing_submission")
        {
            break Some(signal);
        }
        if Instant::now() >= until {
            break None;
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let signal = signal.expect("settlement without an authoritative submission raises attention");
    assert_eq!(signal.detail["run_id"], run["id"]);
    assert_eq!(signal.detail["ticket_id"], ticket);
    assert_eq!(
        h.core.query("run.list", &json!({"project_id": 1})).unwrap()["runs"][0]["status"],
        "executing"
    );
    assert!(
        h.core
            .query("submission.list", &json!({"project_id": 1}))
            .unwrap()["submissions"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let capability: i64 = rusqlite::Connection::open(&h.database_path)
        .unwrap()
        .query_row(
            "SELECT id FROM capabilities WHERE dispatch_request_id = ?1",
            [queued["id"].as_i64().unwrap()],
            |r| r.get(0),
        )
        .unwrap();
    h.core.command("submission.submit", &json!({
        "mutation": mutation(1, "late-result"), "run_id": run["id"], "capability_id": capability,
        "result": {"kind": "implementation", "tip": "a".repeat(40), "summary": "Result arrived late"},
    })).unwrap();
    assert!(
        observer
            .attention_signals(1)
            .iter()
            .all(|s| s.reason != "missing_submission"),
        "an authoritative result clears the current missing-submission alert"
    );
    observer.shutdown();
}
