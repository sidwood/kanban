mod support;

use kanban_domain::HerdrSession;
use kanban_herdr::fixture::{ScriptedSession, SessionScript};
use kanban_herdr::{SessionClient, SessionMapping};
use tempfile::TempDir;

fn named_mapping(session: &str, product_workspace: &str) -> SessionMapping {
    SessionMapping::new(
        HerdrSession::named(session).expect("the name validates"),
        product_workspace,
        "kanban.seed",
    )
}

#[test]
fn snapshot_connect_verifies_the_workspace_mapping() {
    let dir = TempDir::new().expect("a scratch directory is available");
    let _fixture = ScriptedSession::bind(
        dir.path(),
        "kanban-main",
        "/workspaces/kanban.seed",
        SessionScript::default(),
    );
    let mapping = named_mapping("kanban-main", "/workspaces/kanban.seed");
    let mut client = SessionClient::connect(mapping, dir.path())
        .expect("the session connects through its socket");
    let snapshot = client.snapshot().expect("a snapshot captures full state");

    assert_eq!(snapshot.session, "kanban-main");
    assert_eq!(snapshot.product_workspace, "/workspaces/kanban.seed");
    assert_eq!(snapshot.herdr_workspace, "kanban.seed");
}

#[test]
fn snapshot_refuses_a_product_workspace_mismatch() {
    let dir = TempDir::new().expect("a scratch directory is available");
    let _fixture = ScriptedSession::bind(
        dir.path(),
        "kanban-main",
        "/workspaces/other.seed",
        SessionScript::default(),
    );
    let mapping = named_mapping("kanban-main", "/workspaces/kanban.seed");
    let refusal = SessionClient::connect(mapping, dir.path());

    assert!(
        refusal.is_err(),
        "a mismatched workspace must refuse connection"
    );
}

#[test]
fn snapshot_refuses_a_herdr_workspace_mismatch() {
    let dir = TempDir::new().expect("a scratch directory is available");
    let _fixture = ScriptedSession::bind_with_workspace(
        dir.path(),
        "kanban-main",
        "/workspaces/kanban.seed",
        "other.seed",
        SessionScript::default(),
    );
    let mapping = named_mapping("kanban-main", "/workspaces/kanban.seed");
    let refusal = SessionClient::connect(mapping, dir.path());

    assert!(
        refusal.is_err(),
        "a snapshot serving a different Herdr workspace must refuse connection"
    );
}

#[test]
fn open_leaves_the_snapshot_handshake_to_the_caller() {
    let dir = TempDir::new().expect("a scratch directory is available");
    let _fixture = ScriptedSession::bind(
        dir.path(),
        "kanban-main",
        "/workspaces/kanban.seed",
        SessionScript::default(),
    );
    let mapping = named_mapping("kanban-main", "/workspaces/kanban.seed");
    let mut client = SessionClient::open(mapping, dir.path()).expect("the session socket opens");

    let snapshot = client
        .snapshot()
        .expect("the caller-driven handshake answers");
    client
        .mapping()
        .verify_snapshot(&snapshot)
        .expect("the mapping verifies");
    assert_eq!(snapshot.session, "kanban-main");
}
