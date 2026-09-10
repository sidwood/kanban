//! Planning truth through the serving core (KAN-T140-AC1): the real
//! Unix socket transport, the production handlers, and a disposable
//! SQLite database. The coverage-gap diagnostics measure a member
//! Spec's scope against the claims of its own attached Tickets, so a
//! covered member carries no gap and a genuinely uncovered one keeps
//! blocking (T16); and the human graph gate the planning surface
//! drives records a proposal, refuses an uncovered one, approves a
//! complete one, and leaves every member pinned across a service
//! restart (T23).

use serde_json::{Value, json};
use tempfile::TempDir;

use crate::test_client::{Client, boot};

fn mutation(version: u64, key: &str) -> Value {
    json!({ "optimistic_version": version, "idempotency_key": key })
}

fn id(record: &Value) -> u64 {
    record["id"]
        .as_u64()
        .expect("the record carries an identity")
}

/// Register one Project over a scratch repository.
fn register_project(client: &mut Client, dir: &TempDir, code: &str) {
    let repository = dir.path().join(code.to_lowercase());
    std::fs::create_dir_all(repository.join(".git")).expect("the scratch repository exists");
    client.command(
        "project.register",
        json!({
            "mutation": mutation(0, "register-project"),
            "code": code,
            "name": "Control plane",
            "repository": repository.to_str().expect("the path is UTF-8"),
            "seed_workspace": format!("/workspaces/{}.seed", code.to_lowercase()),
            "default_branch": "main",
            "herdr_workspace": format!("{}.seed", code.to_lowercase()),
            "herdr_session": null,
        }),
    );
}

/// The PRD wire content, varied by the story section it claims.
fn content(user_stories: &str) -> Value {
    json!({
        "name": "Plans and specifications",
        "short_description": "Versioned Plan graphs of Specs",
        "problem_statement": "Planning must survive change without losing truth.",
        "solution": "Enforced story coverage.",
        "user_stories": user_stories,
        "implementation_decisions": "The gate is consumed by graph approval.",
        "testing_decisions": "Application tests prove the gate refuses gaps.",
        "out_of_scope": "The Ticket graph proposal.",
        "further_notes": "None",
    })
}

/// Author one Spec claiming the stories given, returning its identity
/// and its minted number.
fn spec(client: &mut Client, user_stories: &str, key: &str) -> (u64, u64) {
    let created = client.command(
        "spec.create",
        json!({
            "mutation": mutation(0, key),
            "project_id": 1,
            "content": content(user_stories),
        }),
    );
    (
        id(&created),
        created["number"]
            .as_u64()
            .expect("the minted number is a number"),
    )
}

/// One Implementation Ticket attached to `spec`, claiming one
/// criterion per story named.
fn implementation(client: &mut Client, spec: u64, stories: &[&str], key: &str) -> u64 {
    let criteria: Vec<Value> = stories
        .iter()
        .map(|story| json!({ "outcome": format!("{story} is delivered."), "stories": [story] }))
        .collect();
    id(&client.command(
        "ticket.create",
        json!({
            "mutation": mutation(0, key),
            "project_id": 1,
            "kind": "implementation",
            "priority": "normal",
            "spec_id": spec,
            "slice": "Deliver the claimed stories end to end.",
            "criteria": criteria,
        }),
    ))
}

/// A draft Plan over the member Specs given, returning its identity.
fn plan_over(client: &mut Client, members: &[u64]) -> u64 {
    let created = client.command(
        "plan.create",
        json!({ "mutation": mutation(0, "plan-create"), "project_id": 1 }),
    );
    let plan = id(&created);
    let mut version = created["version"]
        .as_u64()
        .expect("the version is a number");
    for member in members {
        let response = client.command(
            "plan.spec.add",
            json!({
                "mutation": mutation(version, &format!("plan-add-{member}")),
                "plan_id": plan,
                "spec_number": member,
            }),
        );
        version = response["version"]
            .as_u64()
            .expect("the version is a number");
    }
    plan
}

fn diagnose(client: &mut Client, plan: u64) -> Value {
    client.query_with(
        "plan.diagnostics",
        json!({ "plan_id": plan, "version": null }),
    )
}

/// The stories Spec 1 claims.
const STORIES_ONE: &str = "\
- CORE-S1-US1: As an operator, I want linked criteria.
- CORE-S1-US2: As an operator, I want covered stories.
";

/// The story Spec 2 claims.
const STORIES_TWO: &str = "\
- CORE-S2-US1: As an operator, I want a gate before execution.
";

#[test]
fn coverage_diagnostics_read_the_members_own_claims_over_the_socket() {
    let dir = TempDir::new().expect("a scratch directory is available");
    let core = boot(&dir);
    let mut client = Client::connect(core.socket_path());
    register_project(&mut client, &dir, "CORE");
    let (covered, covered_number) = spec(&mut client, STORIES_ONE, "spec-one");
    let (_, bare_number) = spec(&mut client, STORIES_TWO, "spec-two");
    implementation(
        &mut client,
        covered,
        &["CORE-S1-US1", "CORE-S1-US2"],
        "ticket-one",
    );
    let plan = plan_over(&mut client, &[covered_number, bare_number]);

    let report = diagnose(&mut client, plan);

    assert_eq!(
        report["coverage_gaps"],
        json!([{
            "spec_number": 2,
            "uncovered": ["CORE-S2-US1"],
            "claims_no_stories": false,
        }]),
        "the member its own Ticket covers carries no gap; the member \
         nothing claims still does: {report}"
    );
    assert_eq!(report["blocking"], json!(true), "{report}");
    core.shutdown();
}

#[test]
fn the_graph_gate_refuses_an_uncovered_graph_and_pins_an_approved_one_across_restart() {
    let dir = TempDir::new().expect("a scratch directory is available");
    let core = boot(&dir);
    let mut client = Client::connect(core.socket_path());
    register_project(&mut client, &dir, "CORE");
    let (spec_id, _) = spec(&mut client, STORIES_ONE, "spec-one");
    client.command(
        "spec.version.approve",
        json!({ "mutation": mutation(1, "spec-approve"), "spec_id": spec_id }),
    );
    let partial = implementation(&mut client, spec_id, &["CORE-S1-US1"], "ticket-one");

    // A graph leaving a story unclaimed never reaches approval.
    let uncovered = id(&client.command(
        "ticket.graph.propose",
        json!({
            "mutation": mutation(0, "graph-uncovered"),
            "spec_id": spec_id,
            "spec_version": 1,
            "tickets": [partial],
            "edges": [],
        }),
    ));
    let refusal = client.command_error(
        "ticket.graph.approve",
        json!({ "mutation": mutation(1, "approve-uncovered"), "proposal_id": uncovered }),
    );
    assert_eq!(refusal["code"], "invalid_request", "{refusal}");
    assert!(
        refusal["message"]
            .as_str()
            .expect("the refusal carries a message")
            .contains("S1-US2"),
        "the refusal names the uncovered story: {refusal}"
    );
    assert_eq!(
        client.query_with("ticket.get", json!({ "ticket_id": partial }))["pinned_spec_version"],
        json!(null),
        "a refused approval pins nothing"
    );

    // The complete graph approves and pins every member.
    let rest = implementation(&mut client, spec_id, &["CORE-S1-US2"], "ticket-two");
    let complete = id(&client.command(
        "ticket.graph.propose",
        json!({
            "mutation": mutation(0, "graph-complete"),
            "spec_id": spec_id,
            "spec_version": 1,
            "tickets": [partial, rest],
            "edges": [{ "from_ticket": partial, "to_ticket": rest }],
        }),
    ));
    let approved = client.command(
        "ticket.graph.approve",
        json!({ "mutation": mutation(1, "approve-complete"), "proposal_id": complete }),
    );
    assert_eq!(approved["state"], json!("approved"), "{approved}");
    core.shutdown();

    // The pins are durable: a fresh core over the same database reads
    // the approved proposal and every Ticket's pin back.
    let rebooted = boot(&dir);
    let mut second = Client::connect(rebooted.socket_path());
    let proposals =
        second.query_with("ticket.graph.list", json!({ "spec_id": spec_id }))["proposals"]
            .as_array()
            .expect("the proposals are a list")
            .clone();
    assert_eq!(
        proposals
            .iter()
            .map(|entry| (id(entry), entry["state"].clone()))
            .collect::<Vec<_>>(),
        vec![
            (uncovered, json!("proposed")),
            (complete, json!("approved")),
        ],
        "both proposals survive with the states the gate left them in"
    );
    for ticket in [partial, rest] {
        assert_eq!(
            second.query_with("ticket.get", json!({ "ticket_id": ticket }))["pinned_spec_version"],
            json!(1),
            "Ticket {ticket} keeps the pin the approval wrote"
        );
    }
    rebooted.shutdown();
}
