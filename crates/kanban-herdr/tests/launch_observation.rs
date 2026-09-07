//! Herdr gate for launch observation (KAN-T46-AC3, DR-HB-15, DR-HB-16):
//! the Coordinator launches implementation agents through the
//! session socket prompt path, and the launch is observable through
//! telemetry — never through a direct Kanban launch method.

use kanban_domain::HerdrSession;
use kanban_herdr::fixture::{ScriptedSession, SessionScript};
use kanban_herdr::{HerdrRequest, PromptRequest, SessionClient, SessionMapping};
use serde_json::json;
use tempfile::TempDir;

#[test]
fn launch_observation_delivers_through_the_session_socket_prompt() {
    let dir = TempDir::new().expect("a scratch directory is available");
    let fixture = ScriptedSession::bind(
        dir.path(),
        "kanban-main",
        "/workspaces/kanban.seed",
        SessionScript::default()
            .with_prompt_accepted(true)
            .with_events(vec![json!({
                "kind": "role.opened",
                "role": "implementer",
                "ticket": "KAN-T46",
                "run": "run-1",
            })]),
    );
    let mapping = SessionMapping::new(
        HerdrSession::named("kanban-main").expect("the name validates"),
        "/workspaces/kanban.seed",
        "kanban.seed",
    );
    let mut client = SessionClient::connect(mapping, dir.path())
        .expect("the session connects through its socket");

    let accepted = client
        .prompt(PromptRequest {
            role: "implementer".to_owned(),
            message: "execute the slice".to_owned(),
        })
        .expect("the Coordinator launch is supported per session");
    assert!(accepted);

    client.subscribe().expect("the subscription lands");
    let event = client.read_event().expect("the launch is observable");
    assert_eq!(event["kind"], json!("role.opened"));
    assert_eq!(event["role"], json!("implementer"));
    assert_eq!(event["run"], json!("run-1"));

    let recorded = fixture.recorded_requests();
    assert!(
        recorded.iter().any(|request| matches!(
            request,
            HerdrRequest::Prompt { role, .. } if role == "implementer"
        )),
        "the launch crosses the session socket as a prompt"
    );
}

#[test]
fn launch_observation_never_uses_a_direct_kanban_launch_method() {
    let dir = TempDir::new().expect("a scratch directory is available");
    let fixture = ScriptedSession::bind(
        dir.path(),
        "kanban-main",
        "/workspaces/kanban.seed",
        SessionScript::default().with_prompt_accepted(true),
    );
    let mapping = SessionMapping::new(
        HerdrSession::named("kanban-main").expect("the name validates"),
        "/workspaces/kanban.seed",
        "kanban.seed",
    );
    let mut client = SessionClient::connect(mapping, dir.path())
        .expect("the session connects through its socket");

    client
        .prompt(PromptRequest {
            role: "implementer".to_owned(),
            message: "continue".to_owned(),
        })
        .expect("the Coordinator launch lands");

    for request in fixture.recorded_requests() {
        // Exhaustive: a Launch (or any other) method would fail to
        // compile here, which is how DR-HB-16 stays enforced.
        match request {
            HerdrRequest::Snapshot
            | HerdrRequest::Subscribe
            | HerdrRequest::Wait { .. }
            | HerdrRequest::Prompt { .. }
            | HerdrRequest::Wake { .. } => {}
        }
    }
}
