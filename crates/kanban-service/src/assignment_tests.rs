//! Ordinary implementer assignment through the serving core
//! (KAN-T140-AC5, KAN-S7-US3): the real Unix socket transport, the
//! production handlers, and a disposable SQLite database. An
//! assignment names one catalogue entry and lands on its own — no
//! review configuration exists for the Ticket yet, and none is
//! required — while the review configuration that follows it still
//! applies and survives a restart.

use serde_json::{Value, json};
use tempfile::TempDir;

use crate::test_client::{Client, boot};

fn mutation(version: u64, key: &str) -> Value {
    json!({ "optimistic_version": version, "idempotency_key": key })
}

fn register_project(client: &mut Client, dir: &TempDir) {
    let repository = dir.path().join("core");
    std::fs::create_dir_all(repository.join(".git")).expect("the scratch repository exists");
    client.command(
        "project.register",
        json!({
            "mutation": mutation(0, "register-project"),
            "code": "CORE",
            "name": "Control plane",
            "repository": repository.to_str().expect("the path is UTF-8"),
            "seed_workspace": "/workspaces/kanban.seed",
            "default_branch": "main",
            "herdr_workspace": "kanban.seed",
            "herdr_session": null,
        }),
    );
}

fn define_profile(client: &mut Client, name: &str, harness: &str, model: &str) {
    client.command(
        "profile.define",
        json!({
            "mutation": mutation(0, &format!("define-{name}")),
            "name": name,
            "harness": harness,
            "model": model,
            "effort": "high",
            "usage_pool": "operator",
        }),
    );
}

#[test]
fn an_implementer_assignment_lands_before_any_review_configuration_exists() {
    let dir = TempDir::new().expect("a scratch directory is available");
    let core = boot(&dir);
    let mut client = Client::connect(core.socket_path());
    register_project(&mut client, &dir);
    define_profile(&mut client, "implementer", "claude-code", "opus");
    define_profile(&mut client, "outsider", "codex", "gpt");
    let ticket = client.command(
        "ticket.create",
        json!({
            "mutation": mutation(0, "create-ticket"),
            "project_id": 1,
            "kind": "task",
            "priority": "normal",
            "title": "Archive the old exports",
            "subtype": "operational",
            "mode": "agent",
            "completion": ["The old exports are archived."],
        }),
    );
    let ticket_id = ticket["id"].as_u64().expect("the identity is a number");

    // Nothing has configured this Ticket's review, and the assignment
    // does not wait for one.
    assert_eq!(
        client.query_with("ticket.review.config", json!({ "ticket_id": ticket_id }))["config"],
        json!(null),
        "the Ticket carries no review configuration yet"
    );
    let assigned = client.command(
        "ticket.assign",
        json!({
            "mutation": mutation(1, "assign-implementer"),
            "ticket_id": ticket_id,
            "profile": "implementer",
        }),
    );
    assert_eq!(assigned["profile"], json!("implementer"), "{assigned}");

    // The review configuration is a later, separate decision, and the
    // separation rule it enforces reads the assignment that already
    // stands.
    let configured = client.command(
        "ticket.review.configure",
        json!({
            "mutation": mutation(0, "configure-review"),
            "ticket_id": ticket_id,
            "stages": [{
                "slots": [{
                    "occupant": { "kind": "profile", "name": "outsider" },
                    "requirement": "required",
                }],
            }],
        }),
    );
    assert_eq!(configured["ticket_id"], json!(ticket_id), "{configured}");
    core.shutdown();

    // Both decisions are durable and independent.
    let rebooted = boot(&dir);
    let mut second = Client::connect(rebooted.socket_path());
    assert_eq!(
        second.query_with("ticket.get", json!({ "ticket_id": ticket_id }))["profile"],
        json!("implementer"),
        "the assignment survives the restart"
    );
    assert_eq!(
        second.query_with("ticket.review.config", json!({ "ticket_id": ticket_id }))["config"]["ticket_id"],
        json!(ticket_id),
        "the review configuration survives beside it"
    );
    rebooted.shutdown();
}
