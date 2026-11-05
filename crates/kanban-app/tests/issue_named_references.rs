//! Linear-style names on the operator issue calls.

use kanban_app::{
    CommentIssueTargets, CommentStore, Core, IssueStore, IssueStoreCommentTargets, OperatorSession,
};
use kanban_dto::{IssueListResponse, IssueRecord, IssueSaveResponse};
use kanban_storage::{
    Database, SqliteCommentStore, SqliteIssueStore, SqlitePlanStore, SqliteProjectStore,
    SqliteSpecStore, SqliteTicketStore,
};
use serde_json::{Value, json};
use std::sync::Arc;

mod common;

use common::{harness, mutation};

fn register_issue_stack(core: &mut Core, database: &Database, attachments: &std::path::Path) {
    let projects = Arc::new(SqliteProjectStore::new(database));
    let tickets = Arc::new(SqliteTicketStore::new(database));
    let specs = Arc::new(SqliteSpecStore::new(database));
    let plans = Arc::new(SqlitePlanStore::new(database));
    let issues: Arc<dyn IssueStore> = Arc::new(SqliteIssueStore::new(database));
    let comments: Arc<dyn CommentStore> = Arc::new(SqliteCommentStore::new(database));
    core.register_plans(plans.clone(), projects.clone(), specs.clone())
        .expect("plans register");
    core.register_specs(specs.clone(), projects.clone(), plans)
        .expect("specs register");
    std::fs::create_dir_all(attachments).expect("attachments dir");
    core.register_tickets(
        tickets.clone(),
        projects.clone(),
        specs.clone(),
        Arc::new(kanban_storage::SqliteEvidenceStore::new(
            database,
            attachments.to_path_buf(),
        )),
    )
    .expect("tickets register");
    core.register_issue_operator(
        tickets,
        projects.clone(),
        specs,
        issues.clone(),
        comments.clone(),
    )
    .expect("issue operator registers");
    let comment_targets: Arc<dyn CommentIssueTargets> =
        Arc::new(IssueStoreCommentTargets::new(issues));
    core.register_comments(comments, projects, comment_targets)
        .expect("comments register");
}

fn session() -> OperatorSession {
    let mut h = harness();
    let attachments = h._dir.path().join("attachments");
    register_issue_stack(&mut h.core, &h.database, &attachments);
    OperatorSession::new(Arc::new(h.core))
}

fn call(session: &OperatorSession, name: &str, key: &str, fields: Value) -> Value {
    let mut payload = fields;
    if payload.get("mutation").is_none() {
        payload["mutation"] = mutation(0, key);
    }
    session.call(name, &payload).expect(name)
}

#[test]
fn save_issue_accepts_a_team_code_project_label_and_milestone_name() {
    let session = session();
    call(
        &session,
        "issue.create_label",
        "project-web",
        json!({
            "project_id": 1,
            "name": "Web",
            "color": "#336699",
            "group": "Project",
            "group_single_select": true
        }),
    );
    call(
        &session,
        "issue.create_milestone",
        "milestone-m1",
        json!({ "project_id": 1, "name": "M1 Foundations" }),
    );

    let saved: IssueSaveResponse = serde_json::from_value(call(
        &session,
        "issue.save",
        "named-create",
        json!({
            "team": "CORE",
            "project": "Web",
            "milestone": "M1 Foundations",
            "title": "Named foundations"
        }),
    ))
    .expect("save shape");
    assert!(
        saved.identifier.starts_with("CORE-"),
        "the team code mints a CORE key: {}",
        saved.identifier
    );

    let read: IssueRecord = serde_json::from_value(
        session
            .call("issue.get", &json!({ "identifier": saved.identifier }))
            .expect("issue.get"),
    )
    .expect("issue record");
    assert!(
        read.labels.iter().any(|label| label == "Web"),
        "the Project label is attached: {:?}",
        read.labels
    );
    assert!(
        read.milestone_id.is_some(),
        "the named milestone is attached"
    );

    let listed: IssueListResponse = serde_json::from_value(
        session
            .call("issue.list", &json!({ "project": "Web" }))
            .expect("issue.list"),
    )
    .expect("list shape");
    assert!(
        listed
            .issues
            .iter()
            .any(|issue| issue.identifier == saved.identifier),
        "list_issues project:'Web' returns the created issue: {:?}",
        listed
            .issues
            .iter()
            .map(|issue| issue.identifier.as_str())
            .collect::<Vec<_>>()
    );
}

#[test]
fn assignee_me_stores_the_configured_operator_name() {
    let mut h = harness();
    let attachments = h._dir.path().join("attachments");
    register_issue_stack(&mut h.core, &h.database, &attachments);
    let session = OperatorSession::with_operator_name(Arc::new(h.core), "Ada");
    let saved: IssueSaveResponse = serde_json::from_value(call(
        &session,
        "issue.save",
        "assignee-me",
        json!({ "project_id": 1, "title": "Mine", "assignee": "me" }),
    ))
    .expect("save shape");
    let read: IssueRecord = serde_json::from_value(
        session
            .call("issue.get", &json!({ "identifier": saved.identifier }))
            .expect("issue.get"),
    )
    .expect("issue record");
    assert_eq!(read.assignee.as_deref(), Some("Ada"));
}
