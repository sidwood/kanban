use kanban_herdr::fixture::{ScriptedSession, SessionScript};
use kanban_herdr::{PromptRequest, SessionClient, SessionMapping};
use tempfile::TempDir;

#[test]
fn prompt_delivers_to_one_role_tab() {
    let dir = TempDir::new().expect("a scratch directory is available");
    let _fixture = ScriptedSession::bind(
        dir.path(),
        "kanban-main",
        "/workspaces/kanban.seed",
        SessionScript::default().with_prompt_accepted(true),
    );
    let mapping = SessionMapping::new("kanban-main", "/workspaces/kanban.seed");
    let mut client = SessionClient::connect(mapping, dir.path())
        .expect("the session connects through its socket");

    let accepted = client
        .prompt(PromptRequest {
            role: "implementer".to_owned(),
            message: "continue".to_owned(),
        })
        .expect("prompting is supported per session");

    assert!(accepted);
}
