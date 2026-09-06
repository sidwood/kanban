//! Herdr gate for coordinator waking (KAN-T42-AC3, DR-HB-14,
//! DR-HB-16): the core wakes the Project Coordinator over the per-
//! session socket on dispatch and never launches implementation
//! agents itself.

use kanban_domain::HerdrSession;
use kanban_herdr::fixture::{ScriptedSession, SessionScript};
use kanban_herdr::{COORDINATOR_ROLE, HerdrRequest, SessionClient, SessionMapping, WakeRequest};
use tempfile::TempDir;

fn mapping() -> SessionMapping {
    SessionMapping::new(
        HerdrSession::named("kanban-main").expect("the name validates"),
        "/workspaces/kanban.seed",
        "kanban.seed",
    )
}

#[test]
fn wake_delivers_to_the_project_coordinator_over_the_session_socket() {
    let dir = TempDir::new().expect("a scratch directory is available");
    let fixture = ScriptedSession::bind(
        dir.path(),
        "kanban-main",
        "/workspaces/kanban.seed",
        SessionScript::default().with_wake_accepted(true),
    );
    let mut client = SessionClient::connect(mapping(), dir.path())
        .expect("the session connects through its socket");

    let accepted = client
        .wake_coordinator(WakeRequest {
            dispatch_request_id: 17,
        })
        .expect("waking the Coordinator is supported per session");

    assert!(accepted);
    let recorded = fixture
        .recorded_requests()
        .into_iter()
        .find(|request| matches!(request, HerdrRequest::Wake { .. }))
        .expect("the wake crossed the session socket");
    assert_eq!(
        recorded,
        HerdrRequest::Wake {
            role: COORDINATOR_ROLE.to_owned(),
            dispatch_request_id: 17,
        }
    );
}

#[test]
fn wake_never_launches_an_implementation_agent() {
    let dir = TempDir::new().expect("a scratch directory is available");
    let fixture = ScriptedSession::bind(
        dir.path(),
        "kanban-main",
        "/workspaces/kanban.seed",
        SessionScript::default().with_wake_accepted(true),
    );
    let mut client = SessionClient::connect(mapping(), dir.path())
        .expect("the session connects through its socket");

    client
        .wake_coordinator(WakeRequest {
            dispatch_request_id: 4,
        })
        .expect("the Coordinator wake lands");

    let recorded = fixture.recorded_requests();
    assert!(
        recorded.iter().any(|request| matches!(
            request,
            HerdrRequest::Wake { role, .. } if role == COORDINATOR_ROLE
        )),
        "dispatch wakes the Coordinator over the session socket"
    );
    assert!(
        recorded.iter().all(|request| !matches!(
            request,
            HerdrRequest::Prompt { role, .. } if role != COORDINATOR_ROLE
        )),
        "Kanban never prompts an implementation agent as a substitute for waking the Coordinator"
    );
    for request in &recorded {
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
