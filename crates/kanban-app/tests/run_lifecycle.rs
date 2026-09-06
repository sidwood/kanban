//! App gate for run profile snapshots (KAN-S9-US3, DR-EP-04): the run
//! a claimed Dispatch Request mints freezes the requested and the
//! effective profile values — including the fallback transitions the
//! effective resolution walked — and no later catalogue change
//! rewrites them (DR-EP-05).

use kanban_app::Core;
use serde_json::{Value, json};

mod common;

use common::{assign_lane, harness, insert_ticket, insert_ticket_with_profile, mutation};

/// Define one catalogue entry beside the seeded `standard`, naming its
/// fallback by reference.
fn insert_profile(
    database_path: &std::path::Path,
    name: &str,
    model: &str,
    fallback: Option<&str>,
) {
    let conn = rusqlite::Connection::open(database_path).expect("the database reopens");
    conn.execute(
        "INSERT INTO execution_profiles
             (name, harness, model, effort, usage_pool, fallback, version)
         VALUES (?1, 'claude-code', ?2, 'high', 'operator', ?3, 1)",
        rusqlite::params![name, model, fallback],
    )
    .expect("the fixture profile lands");
}

/// Retire one catalogue entry: the run-time state the fallback policy
/// reacts to.
fn retire_profile(database_path: &std::path::Path, name: &str) {
    let conn = rusqlite::Connection::open(database_path).expect("the database reopens");
    conn.execute(
        "UPDATE execution_profiles
         SET retired = 1, retired_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
             version = version + 1
         WHERE name = ?1",
        rusqlite::params![name],
    )
    .expect("the fixture retire lands");
}

/// Queue and claim a Dispatch Request for `ticket`, answering the
/// claimed identity. Each mutation spends its own idempotency key.
fn claimed_request(core: &Core, ticket: u64, key: &str) -> Value {
    let created = core
        .command(
            "dispatch.request",
            &json!({
                "mutation": mutation(0, format!("{key}-create")),
                "ticket_id": ticket,
            }),
        )
        .expect("the request is created");
    let id = created["id"].as_u64().expect("the identity is a number");
    core.command(
        "dispatch.claim",
        &json!({
            "mutation": mutation(1, format!("{key}-claim")),
            "dispatch_request_id": id,
        }),
    )
    .expect("the claim is attempted")
}

/// Acknowledge the run of one claimed Dispatch Request.
fn acknowledge(
    core: &Core,
    dispatch_request_id: u64,
    version: u64,
    key: &str,
) -> Result<Value, Value> {
    core.command(
        "run.acknowledge",
        &json!({
            "mutation": mutation(version, key),
            "dispatch_request_id": dispatch_request_id,
        }),
    )
    .map_err(|error| serde_json::to_value(error).expect("the refusal encodes"))
}

#[test]
fn run_lifecycle_snapshots_requested_and_effective_profiles() {
    let harness = harness();
    // `nightly` names `standard` as its fallback and retires before
    // dispatch, so the run must fall back to run `standard`'s values.
    insert_profile(&harness.database_path, "nightly", "opus", Some("standard"));
    retire_profile(&harness.database_path, "nightly");
    let ticket = insert_ticket_with_profile(&harness.database_path, 1, "normal", "nightly");
    assign_lane(&harness.database_path, ticket);
    let claimed = claimed_request(&harness.core, ticket, "key-dispatch");
    assert_eq!(claimed["claimed"], json!(true), "the claim wins");
    let request_id = claimed["request"]["id"].as_u64().expect("the identity");

    let run = acknowledge(&harness.core, request_id, 2, "key-run").expect("the run acknowledges");

    assert_eq!(run["status"], json!("executing"));
    assert_eq!(run["dispatch_request_id"], json!(request_id));
    assert_eq!(run["ticket_id"], json!(ticket));
    assert_eq!(
        run["requested"],
        json!({
            "name": "nightly",
            "harness": "claude-code",
            "model": "opus",
            "effort": "high",
            "usage_pool": "operator",
        }),
        "the requested snapshot freezes what the assignment named"
    );
    assert_eq!(
        run["effective"],
        json!({
            "name": "standard",
            "harness": "claude-code",
            "model": "opus",
            "effort": "high",
            "usage_pool": "operator",
        }),
        "the effective snapshot freezes what the fallback policy ran"
    );
    assert_eq!(run["fallback"], json!(true));
    assert_eq!(run["fallback_path"], json!(["nightly", "standard"]));

    // The mint lands on the Project's timeline as a run event.
    let conn = rusqlite::Connection::open(&harness.database_path).expect("the database reopens");
    let (kind, action): (String, String) = conn
        .query_row(
            "SELECT kind, json_extract(detail, '$.action') FROM timeline_events
             WHERE json_extract(detail, '$.run_id') = ?1",
            rusqlite::params![run["id"].as_u64().expect("the run identity") as i64],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("the acknowledge is audited");
    assert_eq!(kind, "run");
    assert_eq!(action, "acknowledged");
}

#[test]
fn run_lifecycle_snapshots_an_active_request_without_fallback() {
    let harness = harness();
    let ticket = insert_ticket(&harness.database_path, 1, "normal");
    assign_lane(&harness.database_path, ticket);
    let claimed = claimed_request(&harness.core, ticket, "key-dispatch");
    let request_id = claimed["request"]["id"].as_u64().expect("the identity");

    let run = acknowledge(&harness.core, request_id, 2, "key-run").expect("the run acknowledges");

    assert_eq!(run["requested"]["name"], json!("standard"));
    assert_eq!(run["effective"]["name"], json!("standard"));
    assert_eq!(run["effective"]["model"], json!("opus"));
    assert_eq!(run["fallback"], json!(false));
    assert_eq!(run["fallback_path"], json!([]));
}

#[test]
fn run_lifecycle_snapshots_survive_catalogue_changes() {
    let harness = harness();
    let ticket = insert_ticket(&harness.database_path, 1, "normal");
    assign_lane(&harness.database_path, ticket);
    let claimed = claimed_request(&harness.core, ticket, "key-dispatch");
    let request_id = claimed["request"]["id"].as_u64().expect("the identity");
    acknowledge(&harness.core, request_id, 2, "key-run").expect("the run acknowledges");

    // The catalogue moves on: the snapshotted entry is redefined under
    // the same name. The run keeps the values it froze (DR-EP-05).
    let conn = rusqlite::Connection::open(&harness.database_path).expect("the database reopens");
    conn.execute(
        "UPDATE execution_profiles SET model = 'sonnet', version = version + 1
         WHERE name = 'standard'",
        rusqlite::params![],
    )
    .expect("the redefine lands");

    let listed = harness
        .core
        .query("run.list", &json!({ "project_id": 1 }))
        .expect("the listing serves");
    assert_eq!(
        listed["runs"].as_array().map(Vec::len),
        Some(1),
        "the run is listed: {listed}"
    );
    assert_eq!(listed["runs"][0]["requested"]["model"], json!("opus"));
    assert_eq!(listed["runs"][0]["effective"]["model"], json!("opus"));
}

#[test]
fn run_lifecycle_snapshots_refuse_a_request_that_never_claimed() {
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
    let request_id = created["id"].as_u64().expect("the identity");

    let refusal = acknowledge(&harness.core, request_id, 1, "key-run")
        .expect_err("a queued request has no run");

    assert_eq!(refusal["code"], json!("invalid_request"));
    assert!(
        refusal["message"]
            .as_str()
            .expect("the message is text")
            .contains("claimed"),
        "the refusal names the claim rule: {refusal}"
    );
}

#[test]
fn run_lifecycle_snapshots_refuse_a_second_run_for_one_request() {
    let harness = harness();
    let ticket = insert_ticket(&harness.database_path, 1, "normal");
    assign_lane(&harness.database_path, ticket);
    let claimed = claimed_request(&harness.core, ticket, "key-dispatch");
    let request_id = claimed["request"]["id"].as_u64().expect("the identity");
    acknowledge(&harness.core, request_id, 2, "key-run-first").expect("the first run acknowledges");

    let refusal = acknowledge(&harness.core, request_id, 2, "key-run-second")
        .expect_err("one executing run per request");

    assert_eq!(refusal["code"], json!("invalid_request"));
    assert!(
        refusal["message"]
            .as_str()
            .expect("the message is text")
            .contains("already"),
        "the refusal names the duplicate: {refusal}"
    );
}

#[test]
fn run_lifecycle_snapshots_refuse_an_unresolvable_profile() {
    let harness = harness();
    // The assigned entry retires naming no fallback: nothing effective
    // exists to run.
    insert_profile(&harness.database_path, "bare", "haiku", None);
    retire_profile(&harness.database_path, "bare");
    let ticket = insert_ticket_with_profile(&harness.database_path, 1, "normal", "bare");
    assign_lane(&harness.database_path, ticket);
    let claimed = claimed_request(&harness.core, ticket, "key-dispatch");
    let request_id = claimed["request"]["id"].as_u64().expect("the identity");

    let refusal = acknowledge(&harness.core, request_id, 2, "key-run")
        .expect_err("no effective profile resolves");

    assert_eq!(refusal["code"], json!("invalid_request"));
    assert!(
        refusal["message"]
            .as_str()
            .expect("the message is text")
            .contains("bare"),
        "the refusal names the profile that cannot run: {refusal}"
    );
}
