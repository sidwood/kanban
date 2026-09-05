mod support;

use kanban_domain::HerdrSession;
use kanban_herdr::fixture::{ScriptedSession, SessionScript};
use kanban_herdr::{SessionClient, SessionMapping, WaitRequest};
use serde_json::json;
use tempfile::TempDir;

#[test]
fn subscribe_receives_scripted_push_events() {
    let dir = TempDir::new().expect("a scratch directory is available");
    let _fixture = ScriptedSession::bind(
        dir.path(),
        "kanban-main",
        "/workspaces/kanban.seed",
        SessionScript::default().with_events(vec![json!({
            "kind": "role.output",
            "role": "implementer",
            "text": "working"
        })]),
    );
    let mapping = SessionMapping::new(
        HerdrSession::named("kanban-main").expect("the name validates"),
        "/workspaces/kanban.seed",
        "kanban.seed",
    );
    let mut client = SessionClient::connect(mapping, dir.path())
        .expect("the session connects through its socket");

    client
        .subscribe()
        .expect("subscription starts on the socket");
    let event = client
        .read_event()
        .expect("push events arrive after subscribe");

    assert_eq!(event["kind"], json!("role.output"));
    assert_eq!(event["role"], json!("implementer"));
}

#[test]
fn wait_returns_the_scripted_result() {
    let dir = TempDir::new().expect("a scratch directory is available");
    let _fixture = ScriptedSession::bind(
        dir.path(),
        "kanban-main",
        "/workspaces/kanban.seed",
        SessionScript::default().with_wait(true, json!({ "role": "implementer" })),
    );
    let mapping = SessionMapping::new(
        HerdrSession::named("kanban-main").expect("the name validates"),
        "/workspaces/kanban.seed",
        "kanban.seed",
    );
    let mut client = SessionClient::connect(mapping, dir.path())
        .expect("the session connects through its socket");

    let (met, detail) = client
        .wait(WaitRequest {
            condition: "role.settled".to_owned(),
            timeout_ms: 500,
        })
        .expect("waiting is supported per session");

    assert!(met);
    assert_eq!(detail["role"], json!("implementer"));
}
