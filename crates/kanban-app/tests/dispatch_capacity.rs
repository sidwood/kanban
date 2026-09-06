//! App gate for counting a dispatch candidate once (KAN-T127,
//! KAN-S7-US3): a candidate is included exactly once on every
//! capacity dimension, so an assigned candidate at the exact global,
//! Project, or profile limit claims, and a second identical candidate
//! is refused on the true counts — never on double-count slack.

use kanban_app::Core;
use serde_json::{Value, json};

mod common;

use common::{
    assign_lane, cap_project, constrain_global, harness, insert_ticket, insert_ticket_with_profile,
    mutation,
};

/// Queue one Dispatch Request and answer its identity.
fn enqueue(core: &Core, ticket: u64, key: &str) -> u64 {
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
fn claim(core: &Core, dispatch_request_id: u64, key: &str) -> Value {
    core.command(
        "dispatch.claim",
        &json!({
            "mutation": mutation(1, key),
            "dispatch_request_id": dispatch_request_id,
        }),
    )
    .expect("the claim is attempted")
}

/// Queue and claim for `ticket`, proving the claim/refusal contract
/// at the exact limit: the first candidate exactly fills the last
/// slot and a second identical candidate is refused naming the one
/// active run the first became.
fn exact_limit_pair(harness: &common::DispatchHarness, refusal: &str) {
    let assigned = insert_ticket(&harness.database_path, 1, "normal");
    assign_lane(&harness.database_path, assigned);
    let identical = insert_ticket(&harness.database_path, 2, "normal");

    let first = claim(
        &harness.core,
        enqueue(&harness.core, assigned, "key-create-first"),
        "key-claim-first",
    );
    assert_eq!(
        first["claimed"],
        json!(true),
        "the assigned candidate at the exact limit is counted once"
    );

    let second = claim(
        &harness.core,
        enqueue(&harness.core, identical, "key-create-second"),
        "key-claim-second",
    );
    assert_eq!(second["claimed"], json!(false));
    assert_eq!(second["request"]["status"], json!("queued"));
    assert_eq!(
        second["capacity_refusal"],
        json!(refusal),
        "the refusal names the one active run, not double-count slack"
    );
}

#[test]
fn an_assigned_candidate_at_the_exact_global_limit_claims_per_dimension() {
    let dimensions = [
        (
            "max_active_per_harness",
            "1 active runs on harness `claude-code` already meet the cap 1",
        ),
        (
            "max_active_per_model",
            "1 active runs on model family `opus` already meet the cap 1",
        ),
        (
            "max_active_per_usage_pool",
            "1 active runs in usage pool `operator` already meet the cap 1",
        ),
    ];
    for (dimension, refusal) in dimensions {
        let harness = harness();
        constrain_global(&harness.database_path, dimension, 1);
        exact_limit_pair(&harness, refusal);
    }
}

#[test]
fn an_assigned_candidate_at_the_exact_project_limit_claims_per_dimension() {
    let dimensions = [
        (
            "max_active_per_harness",
            "1 active runs on harness `claude-code` already meet the cap 1",
        ),
        (
            "max_active_per_model",
            "1 active runs on model family `opus` already meet the cap 1",
        ),
        (
            "max_active_per_usage_pool",
            "1 active runs in usage pool `operator` already meet the cap 1",
        ),
    ];
    for (dimension, refusal) in dimensions {
        let harness = harness();
        cap_project(&harness.database_path, dimension, 1);
        exact_limit_pair(&harness, refusal);
    }
}

#[test]
fn an_assigned_candidate_at_the_exact_lane_cap_claims() {
    let harness = harness();
    cap_project(&harness.database_path, "max_active_lanes", 2);
    let assigned = insert_ticket(&harness.database_path, 1, "normal");
    assign_lane(&harness.database_path, assigned);
    // A second Lane holds another Ticket, so the Project's two active
    // Lanes already meet the cap of 2.
    let other = insert_ticket(&harness.database_path, 2, "normal");
    assign_lane(&harness.database_path, other);

    let claimed = claim(
        &harness.core,
        enqueue(&harness.core, assigned, "key-create"),
        "key-claim",
    );

    assert_eq!(
        claimed["claimed"],
        json!(true),
        "the Lane already holding the candidate's Ticket is not counted on top of the candidate"
    );
}

#[test]
fn a_second_identical_candidate_is_refused_without_double_count_slack() {
    let harness = harness();
    cap_project(&harness.database_path, "max_active_lanes", 1);
    let assigned = insert_ticket(&harness.database_path, 1, "normal");
    assign_lane(&harness.database_path, assigned);
    let identical = insert_ticket(&harness.database_path, 2, "normal");

    let first = claim(
        &harness.core,
        enqueue(&harness.core, assigned, "key-create-first"),
        "key-claim-first",
    );
    assert_eq!(
        first["claimed"],
        json!(true),
        "the assigned candidate at the exact cap is counted once"
    );

    let second = claim(
        &harness.core,
        enqueue(&harness.core, identical, "key-create-second"),
        "key-claim-second",
    );
    assert_eq!(second["claimed"], json!(false));
    assert_eq!(
        second["capacity_refusal"],
        json!("1 active Lanes already meet the maximum 1"),
        "the refusal names the one other active Lane, not double-count slack"
    );
}

#[test]
fn the_model_dimension_keys_verbatim_on_the_profile_model_string() {
    // The profile schema owns no family vocabulary beyond the model
    // string, so capacity never groups distinct strings into one
    // family; the family-key ruling stays pending with the Operator.
    let harness = harness();
    constrain_global(&harness.database_path, "max_active_per_model", 1);
    let conn = rusqlite::Connection::open(&harness.database_path).expect("the database reopens");
    conn.execute(
        "INSERT INTO execution_profiles
             (name, harness, model, effort, usage_pool, version)
         VALUES ('nightly', 'claude-code', 'claude-opus-5', 'high', 'operator', 1)",
        rusqlite::params![],
    )
    .expect("the fixture profile lands");
    let opus = insert_ticket(&harness.database_path, 1, "normal");
    assign_lane(&harness.database_path, opus);
    let nightly = insert_ticket_with_profile(&harness.database_path, 2, "normal", "nightly");
    assign_lane(&harness.database_path, nightly);

    let first = claim(
        &harness.core,
        enqueue(&harness.core, opus, "key-create-opus"),
        "key-claim-opus",
    );
    assert_eq!(first["claimed"], json!(true));

    let second = claim(
        &harness.core,
        enqueue(&harness.core, nightly, "key-create-nightly"),
        "key-claim-nightly",
    );
    assert_eq!(
        second["claimed"],
        json!(true),
        "distinct model strings never share a quota"
    );
}
