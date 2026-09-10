//! Execution admission through the serving core (KAN-T138-AC5,
//! REC-SHIP-022): the real Unix socket transport, the production
//! handlers, and a disposable database. An ineligible Ticket is
//! refused at claim and acknowledgement with the queue, the Ticket,
//! and the run table unchanged; eligible work admits, and both facts
//! survive a service restart.

use serde_json::{Value, json};
use tempfile::TempDir;

use crate::test_client::{Client, boot};

fn mutation(version: u64, key: impl AsRef<str>) -> Value {
    json!({ "optimistic_version": version, "idempotency_key": key.as_ref() })
}

fn id(record: &Value) -> u64 {
    record["id"]
        .as_u64()
        .expect("the record carries an identity")
}

fn version(record: &Value) -> u64 {
    record["version"]
        .as_u64()
        .expect("the record carries a version")
}

/// Register one Project over a scratch repository, and define the
/// shared Execution Profile the first time.
fn register_project(client: &mut Client, dir: &TempDir, code: &str, name: &str) {
    let repository = dir.path().join(code.to_lowercase());
    std::fs::create_dir_all(repository.join(".git")).expect("the scratch repository exists");
    client.command(
        "project.register",
        json!({
            "mutation": mutation(0, format!("register-{code}")),
            "code": code,
            "name": name,
            "repository": repository.to_str().expect("the path is UTF-8"),
            "seed_workspace": format!("/workspaces/{}.seed", code.to_lowercase()),
            "default_branch": "main",
            "herdr_workspace": format!("{}.seed", code.to_lowercase()),
            "herdr_session": null,
        }),
    );
}

fn define_profile(client: &mut Client) {
    client.command(
        "profile.define",
        json!({
            "mutation": mutation(0, "define-standard"),
            "name": "standard",
            "harness": "claude-code",
            "model": "opus",
            "effort": "high",
            "usage_pool": "operator",
        }),
    );
}

fn quick_capture_bug(client: &mut Client, project: u64, key: &str) -> Value {
    client.command(
        "ticket.create",
        json!({
            "mutation": mutation(0, format!("{key}-capture")),
            "project_id": project,
            "kind": "bug",
            "priority": "normal",
            "title": "Claim admits an unqualified Bug",
            "actual_behaviour": "The claim succeeded.",
            "reporter_evidence": "The recovery audit probe transcript.",
        }),
    )
}

/// Qualify `bug` completely, as the operator does before it may leave
/// draft.
fn qualify(client: &mut Client, code: &str, bug: &Value, key: &str) -> Value {
    client.command(
        "ticket.bug.qualify",
        json!({
            "mutation": mutation(version(bug), format!("{key}-qualify")),
            "ticket_id": id(bug),
            "qualification": {
                "expected_behaviour": "Ordinary admission refuses an ineligible Ticket.",
                "reproduction": "Enqueue an unqualified Bug and claim it over the socket.",
                "environment": "macOS 26, disposable SQLite.",
                "severity": "critical",
                "frequency": "Every claim.",
                "affected_scope": "Every dispatch path.",
                "risk": "Unauthorised execution.",
                "criteria": [{
                    "outcome": "The claim is refused and nothing changes.",
                    "stories": [format!("{code}-S1-US1")]
                }],
                "verification_steps": [{ "command": "cargo test -p kanban-service admission" }]
            }
        }),
    )
}

/// Park then unpark: the human commands that carry an agent-owned
/// kind from draft to ready without a drag.
fn park_and_unpark(client: &mut Client, ticket: &Value, key: &str) -> Value {
    let parked = client.command(
        "ticket.park",
        json!({
            "mutation": mutation(version(ticket), format!("{key}-park")),
            "ticket_id": id(ticket),
        }),
    );
    client.command(
        "ticket.unpark",
        json!({
            "mutation": mutation(version(&parked), format!("{key}-unpark")),
            "ticket_id": id(ticket),
        }),
    )
}

fn task(client: &mut Client, project: u64, key: &str) -> Value {
    client.command(
        "ticket.create",
        json!({
            "mutation": mutation(0, format!("{key}-task")),
            "project_id": project,
            "kind": "task",
            "priority": "normal",
            "title": "Bounded operational work",
            "subtype": "operational",
            "mode": "agent",
            "completion": ["The work is done."],
        }),
    )
}

fn make_ready(client: &mut Client, ticket: &Value, key: &str) -> Value {
    client.command(
        "ticket.transition",
        json!({
            "mutation": mutation(version(ticket), format!("{key}-ready")),
            "ticket_id": id(ticket),
            "to": "ready",
        }),
    )
}

fn assign_and_seat(client: &mut Client, project: u64, ticket: &Value, key: &str) {
    client.command(
        "ticket.assign",
        json!({
            "mutation": mutation(version(ticket), format!("{key}-assign")),
            "ticket_id": id(ticket),
            "profile": "standard",
        }),
    );
    let lane = client.command(
        "lane.create",
        json!({
            "mutation": mutation(0, format!("{key}-lane")),
            "project_id": project,
        }),
    );
    client.command(
        "lane.ticket.assign",
        json!({
            "mutation": mutation(version(&lane), format!("{key}-seat")),
            "lane_id": id(&lane),
            "ticket_id": id(ticket),
        }),
    );
}

fn enqueue(client: &mut Client, ticket: u64, key: &str) -> u64 {
    id(&client.command(
        "dispatch.request",
        json!({
            "mutation": mutation(0, format!("{key}-request")),
            "ticket_id": ticket,
        }),
    ))
}

fn claim_payload(request: u64, key: &str) -> Value {
    json!({
        "mutation": mutation(1, format!("{key}-claim")),
        "dispatch_request_id": request,
    })
}

fn queued_ids(client: &mut Client, project: u64) -> Vec<u64> {
    client.query_with("dispatch.queue", json!({ "project_id": project }))["requests"]
        .as_array()
        .expect("the requests are a list")
        .iter()
        .map(id)
        .collect()
}

fn run_ids(client: &mut Client, project: u64) -> Vec<u64> {
    client.query_with("run.list", json!({ "project_id": project }))["runs"]
        .as_array()
        .expect("the runs are a list")
        .iter()
        .map(id)
        .collect()
}

fn ticket_state(client: &mut Client, ticket: u64) -> (String, u64) {
    let record = client.query_with("ticket.get", json!({ "ticket_id": ticket }));
    (
        record["state"]
            .as_str()
            .expect("the state is text")
            .to_owned(),
        version(&record),
    )
}

#[test]
fn admission_refuses_unsafe_claims_over_the_socket_and_admits_legal_work_across_restart() {
    let dir = TempDir::new().expect("a scratch directory is available");
    let core = boot(&dir);
    let mut client = Client::connect(core.socket_path());
    register_project(&mut client, &dir, "WAVE", "Wave pool");
    define_profile(&mut client);

    // An unqualified draft Bug enters the queue but never execution.
    let bug = quick_capture_bug(&mut client, 1, "unqualified");
    assign_and_seat(&mut client, 1, &bug, "unqualified");
    let unqualified = enqueue(&mut client, id(&bug), "unqualified");
    let bug_before = ticket_state(&mut client, id(&bug));
    let refusal = client.command_error("dispatch.claim", claim_payload(unqualified, "unqualified"));
    assert_eq!(refusal["code"], "invalid_request", "{refusal}");
    assert_eq!(
        refusal["message"],
        "the Bug is unqualified; a Bug executes only once its qualification is complete"
    );
    assert_eq!(ticket_state(&mut client, id(&bug)), bug_before);
    assert_eq!(queued_ids(&mut client, 1), vec![unqualified]);
    assert!(run_ids(&mut client, 1).is_empty());
    let ack = client.command_error(
        "run.acknowledge",
        json!({
            "mutation": mutation(1, "unqualified-ack"),
            "dispatch_request_id": unqualified,
        }),
    );
    assert_eq!(ack["code"], "invalid_request", "{ack}");
    assert!(run_ids(&mut client, 1).is_empty());

    // A queued Task cancelled before its claim is refused too.
    let cancelled = task(&mut client, 1, "cancelled");
    let ready = make_ready(&mut client, &cancelled, "cancelled");
    assign_and_seat(&mut client, 1, &ready, "cancelled");
    let stale = enqueue(&mut client, id(&cancelled), "cancelled");
    let (_, current) = ticket_state(&mut client, id(&cancelled));
    client.command(
        "ticket.cancel",
        json!({
            "mutation": mutation(current, "cancelled-cancel"),
            "ticket_id": id(&cancelled),
        }),
    );
    let refusal = client.command_error("dispatch.claim", claim_payload(stale, "cancelled"));
    assert_eq!(refusal["code"], "invalid_request", "{refusal}");
    assert_eq!(
        refusal["message"],
        "cancelled and superseded are terminal; the Ticket accepts no further changes"
    );
    assert_eq!(ticket_state(&mut client, id(&cancelled)).0, "cancelled");
    assert_eq!(queued_ids(&mut client, 1), vec![unqualified, stale]);

    // A ready Task admits, and its run acknowledges.
    let legal = task(&mut client, 1, "legal");
    let ready = make_ready(&mut client, &legal, "legal");
    assign_and_seat(&mut client, 1, &ready, "legal");
    let admitted = enqueue(&mut client, id(&legal), "legal");
    let won = client.command("dispatch.claim", claim_payload(admitted, "legal"));
    assert_eq!(won["claimed"], json!(true), "{won}");
    let run = client.command(
        "run.acknowledge",
        json!({
            "mutation": mutation(version(&won["request"]), "legal-ack"),
            "dispatch_request_id": admitted,
        }),
    );
    assert_eq!(run["status"], "executing");
    assert_eq!(run_ids(&mut client, 1), vec![id(&run)]);
    assert_eq!(queued_ids(&mut client, 1), vec![unqualified, stale]);
    drop(client);
    core.shutdown();

    // Everything above is durable, and the refusals hold on reload.
    let core = boot(&dir);
    let mut client = Client::connect(core.socket_path());
    assert_eq!(run_ids(&mut client, 1), vec![id(&run)]);
    assert_eq!(queued_ids(&mut client, 1), vec![unqualified, stale]);
    let refusal = client.command_error(
        "dispatch.claim",
        claim_payload(unqualified, "unqualified-reloaded"),
    );
    assert_eq!(
        refusal["message"],
        "the Bug is unqualified; a Bug executes only once its qualification is complete"
    );
    let refusal =
        client.command_error("dispatch.claim", claim_payload(stale, "cancelled-reloaded"));
    assert_eq!(
        refusal["message"],
        "cancelled and superseded are terminal; the Ticket accepts no further changes"
    );
    assert_eq!(ticket_state(&mut client, id(&bug)), bug_before);
    assert!(run_ids(&mut client, 1).len() == 1);
    drop(client);
    core.shutdown();
}

#[test]
fn admission_scopes_to_the_owning_project_and_admits_a_qualified_bug_over_the_socket() {
    let dir = TempDir::new().expect("a scratch directory is available");
    let core = boot(&dir);
    let mut client = Client::connect(core.socket_path());
    register_project(&mut client, &dir, "WAVE", "Wave pool");
    register_project(&mut client, &dir, "TIDE", "Tide pool");
    define_profile(&mut client);

    // Project 1 queues work that nothing in Project 2 may disturb.
    let held = task(&mut client, 1, "held");
    let ready = make_ready(&mut client, &held, "held");
    assign_and_seat(&mut client, 1, &ready, "held");
    let held_request = enqueue(&mut client, id(&held), "held");
    let held_before = ticket_state(&mut client, id(&held));

    // A fully qualified Bug is legal work in Project 2: it admits.
    let bug = quick_capture_bug(&mut client, 2, "qualified");
    let qualified = qualify(&mut client, "TIDE", &bug, "qualified");
    let ready_bug = park_and_unpark(&mut client, &qualified, "qualified");
    assert_eq!(ready_bug["state"], "ready", "{ready_bug}");
    assign_and_seat(&mut client, 2, &ready_bug, "qualified");
    let admitted = enqueue(&mut client, id(&bug), "qualified");
    let won = client.command("dispatch.claim", claim_payload(admitted, "qualified"));
    assert_eq!(won["claimed"], json!(true), "{won}");
    let run = client.command(
        "run.acknowledge",
        json!({
            "mutation": mutation(version(&won["request"]), "qualified-ack"),
            "dispatch_request_id": admitted,
        }),
    );
    assert_eq!(run["status"], "executing");

    // A Task cancelled after enqueue is refused in Project 2 too.
    let doomed = task(&mut client, 2, "doomed");
    let ready_doomed = make_ready(&mut client, &doomed, "doomed");
    assign_and_seat(&mut client, 2, &ready_doomed, "doomed");
    let stale = enqueue(&mut client, id(&doomed), "doomed");
    let (_, current) = ticket_state(&mut client, id(&doomed));
    client.command(
        "ticket.cancel",
        json!({
            "mutation": mutation(current, "doomed-cancel"),
            "ticket_id": id(&doomed),
        }),
    );
    let refusal = client.command_error("dispatch.claim", claim_payload(stale, "doomed"));
    assert_eq!(
        refusal["message"],
        "cancelled and superseded are terminal; the Ticket accepts no further changes"
    );

    assert_eq!(
        queued_ids(&mut client, 1),
        vec![held_request],
        "Project 1 keeps its own queue"
    );
    assert_eq!(queued_ids(&mut client, 2), vec![stale]);
    assert!(
        run_ids(&mut client, 1).is_empty(),
        "no run reaches the Project that admitted nothing"
    );
    assert_eq!(run_ids(&mut client, 2), vec![id(&run)]);
    assert_eq!(ticket_state(&mut client, id(&held)), held_before);
    drop(client);
    core.shutdown();
}
