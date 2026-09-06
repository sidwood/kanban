//! App gate for minting run-scoped capabilities (KAN-S9-US4,
//! DR-HB-17, DR-SS-14): each won Dispatch Request claim mints a
//! capability bound to the Ticket, its Lane, the implementer role,
//! and a permitted MCP operation set narrower than operator
//! authority. The mint rides the claim's transaction; a claim that
//! cannot mint does not claim.

mod common;

use common::{assign_lane, harness, insert_ticket, mutation};
use kanban_app::{
    AGENT_MCP_OPERATIONS, CapabilityStore as _, DispatchStore as _, agent_surface,
    exposed_operations,
};
use kanban_domain::{McpOperations, enforce_within_surface};
use kanban_storage::{SqliteCapabilityStore, SqliteDispatchStore};
use serde_json::{Value, json};

/// Queue one Dispatch Request and answer its identity.
fn enqueue(core: &kanban_app::Core, ticket: u64, key: &str) -> u64 {
    let created = core
        .command(
            "dispatch.request",
            &json!({
                "mutation": mutation(0, key),
                "ticket_id": ticket,
            }),
        )
        .expect("the request is created");
    created["id"].as_u64().expect("the identity is a number")
}

/// Claim one Dispatch Request; a capacity miss is a response, not an
/// error.
fn claim(core: &kanban_app::Core, dispatch_request_id: u64, key: &str) -> Value {
    core.command(
        "dispatch.claim",
        &json!({
            "mutation": mutation(1, key),
            "dispatch_request_id": dispatch_request_id,
        }),
    )
    .expect("the claim is attempted")
}

/// The operations an implementer run holds, in canonical order.
fn expected_operations() -> Vec<Value> {
    AGENT_MCP_OPERATIONS
        .iter()
        .map(|name| json!(name))
        .collect()
}

#[test]
fn capability_mint_binds_the_won_claim_to_ticket_and_lane() {
    let harness = harness();
    let ticket = insert_ticket(&harness.database_path, 1, "normal");
    assign_lane(&harness.database_path, ticket);
    let id = enqueue(&harness.core, ticket, "key-create");

    let won = claim(&harness.core, id, "key-claim");

    assert_eq!(won["claimed"], json!(true));
    let capability = &won["capability"];
    assert_eq!(capability["dispatch_request_id"], json!(id));
    assert_eq!(capability["ticket_id"], json!(ticket));
    assert_eq!(capability["lane_id"], json!(1));
    assert_eq!(capability["role"], json!("implementer"));
    assert!(
        capability.get("reviewer_slot_id").is_none() || capability["reviewer_slot_id"].is_null(),
        "an implementer binds no reviewer slot"
    );
    assert_eq!(capability["status"], json!("active"));
    assert_eq!(
        capability["operations"],
        Value::Array(expected_operations())
    );
    assert!(
        capability["minted_at"].as_u64().is_some(),
        "the mint names its moment"
    );

    let database =
        kanban_storage::Database::open(&harness.database_path).expect("the database reopens");
    let restored = SqliteCapabilityStore::new(&database)
        .find(kanban_domain::CapabilityId::new(
            capability["id"].as_u64().expect("the identity is a number"),
        ))
        .expect("the reload serves")
        .expect("the capability is durable");
    assert_eq!(restored.scope().ticket().value(), ticket);
    assert_eq!(restored.scope().lane().value(), 1);
    assert_eq!(
        restored.operations().iter().collect::<Vec<_>>(),
        AGENT_MCP_OPERATIONS,
        "the stored grant is the canonical permitted set"
    );
}

#[test]
fn capability_mint_refuses_a_ticket_in_no_lane() {
    let harness = harness();
    let ticket = insert_ticket(&harness.database_path, 1, "normal");
    let id = enqueue(&harness.core, ticket, "key-create");

    let error = harness
        .core
        .command(
            "dispatch.claim",
            &json!({
                "mutation": mutation(1, "key-claim"),
                "dispatch_request_id": id,
            }),
        )
        .expect_err("a run executes in a Lane, so a claim with none is refused");

    assert!(
        error.message.contains("Lane"),
        "the refusal names the Lane: {}",
        error.message
    );

    let database =
        kanban_storage::Database::open(&harness.database_path).expect("the database reopens");
    let restored = SqliteDispatchStore::new(&database)
        .find(kanban_domain::DispatchRequestId::new(id))
        .expect("the reload serves")
        .expect("the request is durable");
    assert_eq!(restored.status().wire_name(), "queued");
    let conn = rusqlite::Connection::open(&harness.database_path).expect("the db reopens");
    let rows: i64 = conn
        .query_row("SELECT COUNT(*) FROM capabilities", [], |row| row.get(0))
        .expect("the count serves");
    assert_eq!(rows, 0, "a refused mint leaves no capability row");
}

#[test]
fn capability_mint_leaves_capacity_losers_without_a_capability() {
    let harness = harness();
    common::constrain_harness(&harness.database_path, 1);
    let winner = insert_ticket(&harness.database_path, 1, "normal");
    assign_lane(&harness.database_path, winner);
    let loser = insert_ticket(&harness.database_path, 2, "normal");
    assign_lane(&harness.database_path, loser);

    let first = claim(
        &harness.core,
        enqueue(&harness.core, winner, "key-create-first"),
        "key-claim-first",
    );
    assert_eq!(first["claimed"], json!(true));
    assert!(first["capability"].is_object(), "the winner mints");

    let second = claim(
        &harness.core,
        enqueue(&harness.core, loser, "key-create-second"),
        "key-claim-second",
    );
    assert_eq!(second["claimed"], json!(false));
    assert!(
        second["capability"].is_null(),
        "a request still queued has granted no authority"
    );

    let conn = rusqlite::Connection::open(&harness.database_path).expect("the db reopens");
    let rows: i64 = conn
        .query_row("SELECT COUNT(*) FROM capabilities", [], |row| row.get(0))
        .expect("the count serves");
    assert_eq!(rows, 1, "exactly the winner's capability exists");
}

#[test]
fn capability_mint_grants_only_operations_under_operator_authority() {
    let catalogued: Vec<&str> = exposed_operations()
        .iter()
        .map(|operation| operation.name)
        .collect();

    for name in AGENT_MCP_OPERATIONS {
        assert!(
            catalogued.contains(name),
            "the curated agent operation `{name}` must answer to a catalogued operation"
        );
    }
    assert!(
        catalogued
            .iter()
            .any(|name| !AGENT_MCP_OPERATIONS.contains(name)),
        "operator authority holds operations no capability may ever name"
    );

    let operator_only =
        McpOperations::new(["capacity.defaults.update"]).expect("the fixture grant validates");
    assert!(
        enforce_within_surface(
            &operator_only,
            &agent_surface().expect("the surface serves")
        )
        .is_err(),
        "minting operator authority past the agent surface is refused"
    );
}
