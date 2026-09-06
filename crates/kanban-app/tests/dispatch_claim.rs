//! App gate for atomic Dispatch Request claims (KAN-T42-AC1,
//! KAN-S9-US1, DR-EP-08): requests are durable, exactly one concurrent
//! claimant wins, and losers remain queued.

use std::sync::Arc;
use std::thread;

use kanban_app::DispatchStore;
use kanban_domain::DispatchRequestId;
use kanban_storage::{Database, SqliteDispatchStore};
use serde_json::json;

mod common;

use common::{assign_lane, harness, insert_ticket, mutation};

#[test]
fn dispatch_claim_persists_a_request_across_reopen() {
    let harness = harness();
    let ticket = insert_ticket(&harness.database_path, 1, "normal");
    assign_lane(&harness.database_path, ticket);

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
    let id = created["id"].as_u64().expect("the identity is a number");
    assert_eq!(created["status"], json!("queued"));

    let database = Database::open(&harness.database_path).expect("the database reopens");
    let restored = SqliteDispatchStore::new(&database)
        .find(DispatchRequestId::new(id))
        .expect("the reload serves")
        .expect("the request is durable");
    assert_eq!(restored.id().value(), id);
    assert_eq!(restored.ticket().value(), ticket);
}

#[test]
fn dispatch_claim_lets_exactly_one_concurrent_claimant_win() {
    let harness = harness();
    let ticket = insert_ticket(&harness.database_path, 1, "urgent");
    assign_lane(&harness.database_path, ticket);
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
    let id = created["id"].as_u64().expect("the identity is a number");
    let core = Arc::new(harness.core);

    let mut joins = Vec::new();
    for index in 0..8 {
        let core = core.clone();
        joins.push(thread::spawn(move || {
            core.command(
                "dispatch.claim",
                &json!({
                    "mutation": mutation(1, format!("key-claim-{index}")),
                    "dispatch_request_id": id,
                }),
            )
        }));
    }
    let mut wins = 0;
    let mut losses = 0;
    for join in joins {
        match join.join().expect("the claimant finishes") {
            Ok(response) if response["claimed"] == json!(true) => wins += 1,
            Ok(_) => losses += 1,
            Err(_) => losses += 1,
        }
    }
    assert_eq!(wins, 1, "exactly one concurrent claimant wins");
    assert_eq!(losses, 7, "the other seven do not take the request");

    let database = Database::open(&harness.database_path).expect("the database reopens");
    let restored = SqliteDispatchStore::new(&database)
        .find(DispatchRequestId::new(id))
        .expect("the reload serves")
        .expect("the request is durable");
    assert_eq!(restored.status().wire_name(), "claimed");
}

#[test]
fn dispatch_claim_leaves_capacity_losers_queued() {
    let harness = harness();
    common::constrain_harness(&harness.database_path, 1);
    let mut ids = Vec::new();
    for number in 1..=8 {
        let ticket = insert_ticket(&harness.database_path, number, "normal");
        assign_lane(&harness.database_path, ticket);
        let created = harness
            .core
            .command(
                "dispatch.request",
                &json!({
                    "mutation": mutation(0, format!("key-create-{number}")),
                    "ticket_id": ticket,
                }),
            )
            .expect("the request is created");
        ids.push(created["id"].as_u64().expect("the identity is a number"));
    }
    let core = Arc::new(harness.core);

    let mut joins = Vec::new();
    for (index, id) in ids.iter().copied().enumerate() {
        let core = core.clone();
        joins.push(thread::spawn(move || {
            core.command(
                "dispatch.claim",
                &json!({
                    "mutation": mutation(1, format!("key-claim-{index}")),
                    "dispatch_request_id": id,
                }),
            )
        }));
    }
    let mut wins = 0;
    let mut queued = 0;
    for join in joins {
        let response = join
            .join()
            .expect("the claimant finishes")
            .expect("capacity exhaustion is not an error");
        if response["claimed"] == json!(true) {
            wins += 1;
        } else {
            queued += 1;
            assert_eq!(response["request"]["status"], json!("queued"));
            assert!(
                response["capacity_refusal"].is_string(),
                "a queued loser names the capacity refusal"
            );
        }
    }
    assert_eq!(wins, 1, "one harness slot admits one winner");
    assert_eq!(queued, 7, "losers remain queued");
}
