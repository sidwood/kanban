//! App gate for the Dispatch Request queue (KAN-T42-AC2, KAN-S9-US1):
//! requests without available capacity stay queued in deterministic
//! priority, readiness, age order.

use serde_json::json;

mod common;

use common::{harness, insert_ticket, mutation};

#[test]
fn dispatch_queue_orders_by_priority_then_readiness_then_age() {
    let harness = harness();
    // Age is enqueue order: the blocked urgent request is created
    // last so only priority, not age, can put it first.
    let normal_ready = insert_ticket(&harness.database_path, 1, "normal");
    let high_ready_older = insert_ticket(&harness.database_path, 2, "high");
    let high_ready_newer = insert_ticket(&harness.database_path, 3, "high");
    let high_blocked = insert_ticket(&harness.database_path, 4, "high");
    let urgent_blocked = insert_ticket(&harness.database_path, 5, "urgent");

    // Mark two Tickets blocked with an explicit external blocker so
    // their snapshotted readiness is false.
    for ticket_id in [high_blocked, urgent_blocked] {
        common::insert_blocker(&harness.database_path, ticket_id);
    }

    for (ticket, key) in [
        (normal_ready, "key-normal"),
        (high_ready_older, "key-high-old"),
        (high_ready_newer, "key-high-new"),
        (high_blocked, "key-high-blocked"),
        (urgent_blocked, "key-urgent"),
    ] {
        harness
            .core
            .command(
                "dispatch.request",
                &json!({
                    "mutation": mutation(0, key),
                    "ticket_id": ticket,
                }),
            )
            .expect("the request is created");
    }

    let listed = harness
        .core
        .query("dispatch.queue", &json!({ "project_id": 1 }))
        .expect("the queue serves");
    let ids: Vec<u64> = listed["requests"]
        .as_array()
        .expect("the requests are a list")
        .iter()
        .map(|request| request["ticket_id"].as_u64().expect("a Ticket identity"))
        .collect();
    assert_eq!(
        ids,
        vec![
            urgent_blocked,
            high_ready_older,
            high_ready_newer,
            high_blocked,
            normal_ready
        ],
        "urgent before high before normal; ready before blocked; older before newer"
    );
}

#[test]
fn dispatch_queue_keeps_requests_without_capacity() {
    let harness = harness();
    common::constrain_harness(&harness.database_path, 1);
    let first = insert_ticket(&harness.database_path, 1, "urgent");
    let second = insert_ticket(&harness.database_path, 2, "low");
    for (ticket, key) in [(first, "key-first"), (second, "key-second")] {
        harness
            .core
            .command(
                "dispatch.request",
                &json!({
                    "mutation": mutation(0, key),
                    "ticket_id": ticket,
                }),
            )
            .expect("the request is created");
    }

    let winner = harness
        .core
        .command(
            "dispatch.claim",
            &json!({
                "mutation": mutation(1, "key-claim-first"),
                "dispatch_request_id": 1,
            }),
        )
        .expect("the first claim is attempted");
    assert_eq!(winner["claimed"], json!(true));

    let loser = harness
        .core
        .command(
            "dispatch.claim",
            &json!({
                "mutation": mutation(1, "key-claim-second"),
                "dispatch_request_id": 2,
            }),
        )
        .expect("the second claim is attempted");
    assert_eq!(loser["claimed"], json!(false));
    assert_eq!(loser["request"]["status"], json!("queued"));
    assert_eq!(loser["request"]["priority"], json!("low"));

    let listed = harness
        .core
        .query("dispatch.queue", &json!({ "project_id": 1 }))
        .expect("the queue serves");
    assert_eq!(listed["requests"].as_array().expect("a list").len(), 1);
    assert_eq!(listed["requests"][0]["ticket_id"], json!(second));
    assert_eq!(listed["requests"][0]["status"], json!("queued"));
}

#[test]
fn dispatch_queue_wakes_the_coordinator_on_dispatch() {
    let harness = harness();
    let ticket = insert_ticket(&harness.database_path, 1, "normal");
    let created = harness
        .core
        .command(
            "dispatch.request",
            &json!({
                "mutation": mutation(0, "key-create"),
                "ticket_id": ticket,
            }),
        )
        .expect("the request is created");

    let wakes = harness
        .wake
        .calls
        .lock()
        .expect("the wake log is sound")
        .clone();
    assert_eq!(wakes.len(), 1, "dispatch wakes the Coordinator once");
    assert_eq!(wakes[0].project_id, 1);
    assert_eq!(
        wakes[0].dispatch_request_id,
        created["id"].as_u64().expect("the identity is a number")
    );
    assert_eq!(wakes[0].seed_workspace, "/workspaces/kanban.seed");
    assert_eq!(wakes[0].herdr_workspace, "kanban.seed");
    assert_eq!(wakes[0].herdr_session.as_deref(), Some("kanban-main"));
}
