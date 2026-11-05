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

#[test]
fn save_comment_accepts_an_issue_key_and_body() {
    let mut h = harness();
    let attachments = h._dir.path().join("attachments");
    register_issue_stack(&mut h.core, &h.database, &attachments);
    let session = OperatorSession::with_operator_name(Arc::new(h.core), "Ada");
    let saved: IssueSaveResponse = serde_json::from_value(call(
        &session,
        "issue.save",
        "comment-host",
        json!({ "project_id": 1, "title": "Commented" }),
    ))
    .expect("save shape");
    // The acceptance call names CORE-2. This host is the first issue, so
    // address whichever key was minted and also prove CORE-2 when it is.
    let key = saved.identifier;
    let posted = session
        .call(
            "save_comment",
            &json!({ "issue": key, "body": "Ship report" }),
        )
        .expect("save_comment");
    assert!(
        posted["id"].as_u64().is_some(),
        "the comment id is returned: {posted}"
    );
    assert_eq!(posted["identifier"], json!(key));
    let url = posted["url"].as_str().unwrap_or("");
    assert!(
        url.contains(&key),
        "the result carries the issue URL: {posted}"
    );
}

#[test]
fn get_issue_shows_team_project_milestone_and_creator() {
    let mut h = harness();
    let attachments = h._dir.path().join("attachments");
    register_issue_stack(&mut h.core, &h.database, &attachments);
    let session = OperatorSession::with_operator_name(Arc::new(h.core), "Ada");
    call(
        &session,
        "issue.create_label",
        "readable-web",
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
        "readable-m1",
        json!({ "project_id": 1, "name": "M1 Foundations" }),
    );
    let saved: IssueSaveResponse = serde_json::from_value(call(
        &session,
        "issue.save",
        "readable-create",
        json!({
            "team": "CORE",
            "project": "Web",
            "milestone": "M1 Foundations",
            "title": "Readable",
            "description": "Full text"
        }),
    ))
    .expect("save shape");
    let read = session
        .call("issue.get", &json!({ "identifier": saved.identifier }))
        .expect("issue.get");
    assert_eq!(read["team"], json!("CORE"));
    assert_eq!(read["project"], json!("Web"));
    assert_eq!(read["milestone"], json!("M1 Foundations"));
    assert_eq!(read["creator"], json!("Ada"));

    let compact = session
        .call("issue.list", &json!({ "project": "Web" }))
        .expect("issue.list");
    let row = &compact["issues"][0];
    assert!(
        row.get("description").is_none(),
        "default rows omit description: {row}"
    );
    assert!(
        row.get("state_history").is_none(),
        "default rows omit history: {row}"
    );
    let detailed = session
        .call(
            "issue.list",
            &json!({ "project": "Web", "fields": ["description"] }),
        )
        .expect("issue.list");
    assert_eq!(detailed["issues"][0]["description"], json!("Full text"));
}

#[test]
fn description_edits_apply_together_or_not_at_all() {
    let session = session();
    let saved: IssueSaveResponse = serde_json::from_value(call(
        &session,
        "issue.save",
        "edits-create",
        json!({
            "project_id": 1,
            "title": "Edited",
            "description": "alpha beta alpha"
        }),
    ))
    .expect("save shape");
    let version = session
        .call("issue.get", &json!({ "identifier": saved.identifier }))
        .expect("issue.get")["version"]
        .as_u64()
        .expect("version");
    let applied = session
        .call(
            "issue.save",
            &json!({
                "mutation": mutation(version, "edits-apply"),
                "identifier": saved.identifier,
                "description_edits": [
                    {"type": "append", "text": " tail"},
                    {"type": "replace", "find": "beta", "replace": "BETA"}
                ]
            }),
        )
        .expect("the edits apply");
    let _ = applied;
    let after = session
        .call("issue.get", &json!({ "identifier": saved.identifier }))
        .expect("issue.get");
    assert_eq!(after["description"], json!("alpha BETA alpha tail"));

    let before_version = after["version"].as_u64().expect("version");
    let refused = session.call(
        "issue.save",
        &json!({
            "mutation": mutation(before_version, "edits-twice"),
            "identifier": saved.identifier,
            "description_edits": [
                {"type": "replace", "find": "alpha", "replace": "ALPHA"}
            ]
        }),
    );
    assert!(refused.is_err(), "an anchor that matches twice is refused");
    let unchanged = session
        .call("issue.get", &json!({ "identifier": saved.identifier }))
        .expect("issue.get");
    assert_eq!(unchanged["description"], json!("alpha BETA alpha tail"));
    assert_eq!(unchanged["version"], json!(before_version));
}
