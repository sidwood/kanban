mod support;

use kanban_herdr::fixture::{ScriptedSession, SessionScript};
use kanban_herdr::{SessionClient, SessionMapping};
use tempfile::TempDir;

#[test]
fn snapshot_connect_verifies_the_product_workspace_mapping() {
    let dir = TempDir::new().expect("a scratch directory is available");
    let _fixture = ScriptedSession::bind(
        dir.path(),
        "kanban-main",
        "/workspaces/kanban.seed",
        SessionScript::default(),
    );
    let mapping = SessionMapping::new("kanban-main", "/workspaces/kanban.seed");
    let mut client = SessionClient::connect(mapping, dir.path())
        .expect("the session connects through its socket");
    let snapshot = client.snapshot().expect("a snapshot captures full state");

    assert_eq!(snapshot.session, "kanban-main");
    assert_eq!(snapshot.product_workspace, "/workspaces/kanban.seed");
    assert_eq!(snapshot.herdr_workspace, "kanban.seed");
}

#[test]
fn snapshot_refuses_a_workspace_mismatch() {
    let dir = TempDir::new().expect("a scratch directory is available");
    let _fixture = ScriptedSession::bind(
        dir.path(),
        "kanban-main",
        "/workspaces/other.seed",
        SessionScript::default(),
    );
    let mapping = SessionMapping::new("kanban-main", "/workspaces/kanban.seed");
    let refusal = SessionClient::connect(mapping, dir.path());

    assert!(
        refusal.is_err(),
        "a mismatched workspace must refuse connection"
    );
}
