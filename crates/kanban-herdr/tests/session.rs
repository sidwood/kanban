mod support;

use kanban_domain::HerdrSession;
use kanban_herdr::fixture::{ScriptedSession, SessionScript};
use kanban_herdr::{SessionClient, SessionMapping};
use tempfile::TempDir;

#[test]
fn a_named_session_connects_through_its_own_socket() {
    let dir = TempDir::new().expect("a scratch directory is available");
    let _fixture = ScriptedSession::bind(
        dir.path(),
        "kanban-main",
        "/workspaces/kanban.seed",
        SessionScript::default(),
    );
    let mapping = SessionMapping::new(
        HerdrSession::named("kanban-main").expect("the name validates"),
        "/workspaces/kanban.seed",
        "kanban.seed",
    );

    let mut client = SessionClient::connect(mapping, dir.path())
        .expect("the named session connects through its socket");

    assert_eq!(
        client.snapshot().expect("the snapshot serves").session,
        "kanban-main"
    );
}

#[test]
fn a_project_without_a_session_connects_to_the_default_session_socket() {
    let dir = TempDir::new().expect("a scratch directory is available");
    let _fixture = ScriptedSession::bind_default(
        dir.path(),
        "/workspaces/kanban.seed",
        SessionScript::default(),
    );
    let mapping = SessionMapping::new(
        HerdrSession::Default,
        "/workspaces/kanban.seed",
        "kanban.seed",
    );

    let mut client = SessionClient::connect(mapping, dir.path())
        .expect("the default session connects through its well-known socket");

    assert_eq!(
        client
            .snapshot()
            .expect("the snapshot serves")
            .herdr_workspace,
        "kanban.seed"
    );
}

#[test]
fn identical_workspace_identifiers_resolve_inside_their_own_sessions() {
    let dir = TempDir::new().expect("a scratch directory is available");
    // Two sessions serve the same Herdr workspace identifier mapped to
    // two different product workspaces: identity resolves inside the
    // session the binding selected, never across sessions (DR-HB-19).
    let _alpha = ScriptedSession::bind_with_workspace(
        dir.path(),
        "alpha",
        "/workspaces/alpha.seed",
        "shared.seed",
        SessionScript::default(),
    );
    let _beta = ScriptedSession::bind_with_workspace(
        dir.path(),
        "beta",
        "/workspaces/beta.seed",
        "shared.seed",
        SessionScript::default(),
    );

    let alpha = SessionClient::connect(
        SessionMapping::new(
            HerdrSession::named("alpha").expect("the name validates"),
            "/workspaces/alpha.seed",
            "shared.seed",
        ),
        dir.path(),
    )
    .expect("alpha resolves its own shared.seed");
    let beta = SessionClient::connect(
        SessionMapping::new(
            HerdrSession::named("beta").expect("the name validates"),
            "/workspaces/beta.seed",
            "shared.seed",
        ),
        dir.path(),
    )
    .expect("beta resolves its own shared.seed");

    // The same identifier observed through each session maps back to
    // that session's product workspace, proving the two never merge.
    assert_eq!(
        alpha.mapping().product_workspace(),
        "/workspaces/alpha.seed"
    );
    assert_eq!(beta.mapping().product_workspace(), "/workspaces/beta.seed");
}

#[test]
fn a_workspace_identifier_resolved_against_the_wrong_session_is_refused() {
    let dir = TempDir::new().expect("a scratch directory is available");
    let _fixture = ScriptedSession::bind_with_workspace(
        dir.path(),
        "alpha",
        "/workspaces/alpha.seed",
        "shared.seed",
        SessionScript::default(),
    );
    // The binding expects alpha's workspace through beta's session:
    // beta has no socket at all, so the resolution cannot silently
    // fall through to another session.
    let refusal = SessionClient::connect(
        SessionMapping::new(
            HerdrSession::named("beta").expect("the name validates"),
            "/workspaces/beta.seed",
            "shared.seed",
        ),
        dir.path(),
    );

    assert!(refusal.is_err(), "workspace identity stays session-scoped");
}
