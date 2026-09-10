//! Project settings through the serving core (REC-SHIP-014): the real
//! Unix socket transport, the production handlers, and a disposable
//! SQLite database. An operator corrects the settings a Project owns,
//! the core refuses a stale version and an archived Project, the
//! identity and the anchored paths never move, and everything that
//! landed is read back from a restarted core.

use serde_json::{Value, json};
use tempfile::TempDir;

use crate::test_client::{Client, boot};

fn mutation(version: u64, key: &str) -> Value {
    json!({ "optimistic_version": version, "idempotency_key": key })
}

fn register_project(client: &mut Client, dir: &TempDir) -> String {
    let repository = dir.path().join("core");
    std::fs::create_dir_all(repository.join(".git")).expect("the scratch repository exists");
    let anchor = repository
        .canonicalize()
        .expect("the repository path canonicalises")
        .to_str()
        .expect("the path is UTF-8")
        .to_owned();
    client.command(
        "project.register",
        json!({
            "mutation": mutation(0, "register-project"),
            "code": "CORE",
            "name": "Control plane",
            "repository": anchor.clone(),
            "seed_workspace": "/workspaces/kanban.seed",
            "default_branch": "main",
            "herdr_workspace": "kanban.seed",
            "herdr_session": "kanban-main",
        }),
    );
    anchor
}

fn listed(client: &mut Client, id: u64) -> Value {
    client.query("project.list")["projects"]
        .as_array()
        .expect("the register lists projects")
        .iter()
        .find(|project| project["id"] == json!(id))
        .expect("the Project is listed")
        .clone()
}

#[test]
fn project_settings_land_over_the_socket_and_survive_a_restart() {
    let dir = TempDir::new().expect("a scratch directory is available");
    let core = boot(&dir);
    let mut client = Client::connect(core.socket_path());
    let anchor = register_project(&mut client, &dir);
    let initiative = client.command(
        "initiative.create",
        json!({ "mutation": mutation(0, "create-initiative"), "name": "Recovery" }),
    );
    let initiative_id = initiative["id"].as_u64().expect("the identity is a number");

    let updated = client.command(
        "project.update",
        json!({
            "mutation": mutation(1, "update-project"),
            "project_id": 1,
            "name": "Recovery control plane",
            "default_branch": "trunk",
            "herdr_workspace": "kanban.control",
            "herdr_session": "kanban-control",
            "initiative_id": initiative_id,
        }),
    );

    assert_eq!(
        updated["name"],
        json!("Recovery control plane"),
        "{updated}"
    );
    assert_eq!(updated["default_branch"], json!("trunk"), "{updated}");
    assert_eq!(
        updated["herdr_workspace"],
        json!("kanban.control"),
        "{updated}"
    );
    assert_eq!(
        updated["herdr_session"],
        json!("kanban-control"),
        "{updated}"
    );
    assert_eq!(updated["initiative_id"], json!(initiative_id), "{updated}");
    assert_eq!(updated["version"], json!(2), "{updated}");
    assert_eq!(updated["code"], json!("CORE"), "the code is minted once");
    assert_eq!(
        updated["repository"],
        json!(anchor),
        "a Project never changes the repository it anchors"
    );
    assert_eq!(
        updated["seed_workspace"],
        json!("/workspaces/kanban.seed"),
        "a Project never relocates its Seed Workspace"
    );

    // The refusal is the core's, against the version it holds.
    let stale = client.command_error(
        "project.update",
        json!({
            "mutation": mutation(1, "stale-update"),
            "project_id": 1,
            "name": "Renamed by a stale writer",
            "default_branch": "trunk",
            "herdr_workspace": "kanban.control",
            "herdr_session": "kanban-control",
            "initiative_id": initiative_id,
        }),
    );
    assert_eq!(stale["code"], json!("stale_version"), "{stale}");
    assert_eq!(stale["current_version"], json!(2), "{stale}");

    // The audit row for the change sits on the Project's own timeline.
    let history = client.query_with(
        "timeline.query",
        json!({ "scope": { "project": 1 }, "kinds": ["transition"] }),
    );
    let rows = history["events"]
        .as_array()
        .expect("the timeline serves rows")
        .iter()
        .filter(|row| row["detail"]["action"] == json!("updated"))
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(rows.len(), 1, "exactly one update landed: {history}");
    assert_eq!(
        rows[0]["detail"]["changed"]["default_branch"],
        json!({ "from": "main", "to": "trunk" }),
        "the audit records what moved: {history}"
    );
    core.shutdown();

    let rebooted = boot(&dir);
    let mut second = Client::connect(rebooted.socket_path());
    let stored = listed(&mut second, 1);
    assert_eq!(stored["name"], json!("Recovery control plane"), "{stored}");
    assert_eq!(stored["default_branch"], json!("trunk"), "{stored}");
    assert_eq!(
        stored["herdr_workspace"],
        json!("kanban.control"),
        "{stored}"
    );
    assert_eq!(stored["herdr_session"], json!("kanban-control"), "{stored}");
    assert_eq!(stored["initiative_id"], json!(initiative_id), "{stored}");
    assert_eq!(stored["version"], json!(2), "{stored}");
    rebooted.shutdown();
}

#[test]
fn an_archived_project_refuses_a_settings_change_over_the_socket() {
    let dir = TempDir::new().expect("a scratch directory is available");
    let core = boot(&dir);
    let mut client = Client::connect(core.socket_path());
    register_project(&mut client, &dir);
    client.command(
        "project.archive",
        json!({ "mutation": mutation(1, "archive-project"), "project_id": 1 }),
    );

    let refused = client.command_error(
        "project.update",
        json!({
            "mutation": mutation(2, "update-archived"),
            "project_id": 1,
            "name": "Renamed after archiving",
            "default_branch": "main",
            "herdr_workspace": "kanban.seed",
            "herdr_session": "kanban-main",
            "initiative_id": null,
        }),
    );

    assert_eq!(refused["code"], json!("invalid_request"), "{refused}");
    assert!(
        refused["message"]
            .as_str()
            .expect("the refusal carries a message")
            .contains("terminal"),
        "the refusal says archived is terminal: {refused}"
    );
    assert_eq!(
        listed(&mut client, 1)["name"],
        json!("Control plane"),
        "the refusal changed nothing"
    );
    core.shutdown();
}

#[test]
fn clearing_the_session_and_the_initiative_persists_their_absence() {
    let dir = TempDir::new().expect("a scratch directory is available");
    let core = boot(&dir);
    let mut client = Client::connect(core.socket_path());
    register_project(&mut client, &dir);

    client.command(
        "project.update",
        json!({
            "mutation": mutation(1, "clear-session"),
            "project_id": 1,
            "name": "Control plane",
            "default_branch": "main",
            "herdr_workspace": "kanban.seed",
            "herdr_session": null,
            "initiative_id": null,
        }),
    );
    core.shutdown();

    let rebooted = boot(&dir);
    let mut second = Client::connect(rebooted.socket_path());
    let stored = listed(&mut second, 1);
    assert_eq!(
        stored["herdr_session"],
        json!(null),
        "absence selects Herdr's default session: {stored}"
    );
    assert_eq!(stored["initiative_id"], json!(null), "{stored}");
    rebooted.shutdown();
}
