mod support;

use kanban_domain::HerdrSession;
use kanban_herdr::fixture::{ScriptedSession, SessionScript};
use kanban_herdr::{SessionClient, SessionMapping, WaitRequest};
use serde_json::json;
use tempfile::TempDir;

fn role_events() -> Vec<serde_json::Value> {
    vec![
        json!({
            "kind": "role.opened",
            "role": "implementer",
            "project": "CORE",
            "ticket": "KAN-T40",
            "lane": "in_progress",
            "run": "run-1",
            "harness": "claude-code",
            "model": "opus-5"
        }),
        json!({
            "kind": "role.output",
            "role": "implementer",
            "text": "working"
        }),
        json!({ "kind": "role.settled", "role": "implementer" }),
    ]
}

fn connected_client(dir: &TempDir, script: SessionScript) -> SessionClient {
    let _fixture =
        ScriptedSession::bind(dir.path(), "kanban-main", "/workspaces/kanban.seed", script);
    let mapping = SessionMapping::new(
        HerdrSession::named("kanban-main").expect("the name validates"),
        "/workspaces/kanban.seed",
        "kanban.seed",
    );
    let mut client = SessionClient::connect(mapping, dir.path())
        .expect("the session connects through its socket");
    client.subscribe().expect("the subscription starts");
    client
}

#[test]
fn push_events_arrive_in_order_after_subscribe() {
    let dir = TempDir::new().expect("a scratch directory is available");
    let mut client = connected_client(&dir, SessionScript::default().with_events(role_events()));

    let first = client.read_event().expect("the first event arrives");
    let second = client.read_event().expect("the second event arrives");
    let third = client.read_event().expect("the third event arrives");

    assert_eq!(first["kind"], json!("role.opened"));
    assert_eq!(second["kind"], json!("role.output"));
    assert_eq!(third["kind"], json!("role.settled"));
}

#[test]
fn push_events_during_a_wait_are_kept_in_order() {
    let dir = TempDir::new().expect("a scratch directory is available");
    let mut client = connected_client(
        &dir,
        SessionScript::default()
            .with_events(role_events())
            .with_wait(true, json!({ "role": "implementer" })),
    );

    // The wait goes out before any event is read, so the scripted
    // events overtake its response on the wire: normal operation must
    // proceed without losing or reordering the stream.
    let (met, _detail) = client
        .wait(WaitRequest {
            condition: "role.settled".to_owned(),
            timeout_ms: 500,
        })
        .expect("waiting proceeds while events stream");

    assert!(met);
    assert_eq!(
        client.read_event().expect("the overtaking events survive"),
        role_events()[0]
    );
    assert_eq!(
        client.read_event().expect("order is preserved"),
        role_events()[1]
    );
    assert_eq!(
        client.read_event().expect("the last event follows"),
        role_events()[2]
    );
}

#[test]
fn push_events_during_a_prompt_are_kept() {
    let dir = TempDir::new().expect("a scratch directory is available");
    let mut client = connected_client(
        &dir,
        SessionScript::default()
            .with_events(role_events())
            .with_prompt_accepted(true),
    );

    let accepted = client
        .prompt(kanban_herdr::PromptRequest {
            role: "implementer".to_owned(),
            message: "continue".to_owned(),
        })
        .expect("prompting proceeds while events stream");

    assert!(accepted);
    assert_eq!(
        client
            .read_event()
            .expect("the streamed events survive the prompt"),
        role_events()[0]
    );
}
