//! MCP acceptance scenarios from `mcp-acceptance-tests.md` (Linear parity).
//! Exercises the installation operator surface through `OperatorSession`.

use std::sync::Arc;

use kanban_app::{CommentStore, Core, IssueStore, OperatorSession, ProjectStore};
use kanban_domain::ProjectRegistration;
use kanban_dto::{IssueGetQuery, IssueRecord, IssueSaveResponse};
use kanban_storage::{
    Database, SqliteIssueStore, SqlitePlanStore, SqliteProjectStore, SqliteSpecStore,
    SqliteTicketStore,
};
use kanban_storage::SqliteCommentStore;
use serde_json::json;

mod common;

use common::{harness, mutation};

fn save_payload(version: u64, key: &str, fields: serde_json::Value) -> serde_json::Value {
    let mut object = serde_json::Map::new();
    object.insert("mutation".into(), mutation(version, key));
    if let Some(extra) = fields.as_object() {
        object.extend(extra.clone());
    }
    serde_json::Value::Object(object)
}

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
    core.register_issue_operator(tickets, projects.clone(), specs, issues, comments.clone())
        .expect("issue operator registers");
    core.register_comments(comments, projects)
        .expect("comments register");
}

#[test]
fn acceptance_create_and_read_issue_returns_key_and_fields() {
    let mut h = harness();
    let attachments = h._dir.path().join("attachments");
    std::fs::create_dir_all(&attachments).expect("attachments dir");
    register_issue_stack(&mut h.core, &h.database, &attachments);
    let core = Arc::new(h.core);
    let session = OperatorSession::new(core);

    let save = json!({
        "mutation": mutation(0, "acceptance-create"),
        "project_id": 1,
        "title": "Checklist issue",
        "description": "- [ ] Ship Linear parity\n",
        "state": "Draft",
        "priority": 2,
        "due_date": "2026-12-01",
    });
    let saved: IssueSaveResponse = serde_json::from_value(
        session
            .call("issue.save", &save)
            .expect("save succeeds"),
    )
    .expect("save response shape");
    assert!(saved.identifier.contains('-'));
    assert!(saved.url.starts_with("kanban://issue/"));

    let read: IssueRecord = serde_json::from_value(
        session
            .call(
                "issue.get",
                &serde_json::to_value(IssueGetQuery {
                    identifier: saved.identifier.clone(),
                })
                .unwrap(),
            )
            .expect("get succeeds"),
    )
    .expect("issue record shape");
    assert_eq!(read.identifier, saved.identifier);
    assert!(read.description.contains("Ship Linear parity"));
    assert_eq!(read.state, "Draft");
    assert_eq!(read.priority, 2);
}

#[test]
fn acceptance_free_state_move_appends_without_lifecycle_enforcement() {
    let mut h = harness();
    let attachments = h._dir.path().join("attachments");
    std::fs::create_dir_all(&attachments).expect("attachments dir");
    register_issue_stack(&mut h.core, &h.database, &attachments);
    let core = Arc::new(h.core);
    let session = OperatorSession::new(core);

    let saved: IssueSaveResponse = serde_json::from_value(
        session
            .call(
                "issue.save",
                &save_payload(
                    0,
                    "acceptance-state",
                    json!({
                        "project_id": 1,
                        "title": "State hop",
                        "state": "Draft",
                    }),
                ),
            )
            .expect("create succeeds"),
    )
    .unwrap();

    session
        .call(
            "issue.save",
            &save_payload(
                2,
                "acceptance-state-done",
                json!({
                    "identifier": saved.identifier,
                    "state": "Done",
                }),
            ),
        )
        .expect("done transition succeeds");

    let read: IssueRecord = serde_json::from_value(
        session
            .call(
                "issue.get",
                &serde_json::to_value(IssueGetQuery {
                    identifier: saved.identifier,
                })
                .unwrap(),
            )
            .expect("get succeeds"),
    )
    .unwrap();
    assert_eq!(read.state, "Done");
    assert_eq!(read.state_type, "completed");
}

#[test]
fn acceptance_retired_label_is_rejected() {
    let mut h = harness();
    let attachments = h._dir.path().join("attachments");
    std::fs::create_dir_all(&attachments).expect("attachments dir");
    register_issue_stack(&mut h.core, &h.database, &attachments);
    let conn = rusqlite::Connection::open(&h.database_path).expect("database opens");
    conn.execute(
        "UPDATE issue_labels SET retired_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
         WHERE project_id = 1 AND name = 'task'",
        [],
    )
    .expect("retire label");
    drop(conn);

    let core = Arc::new(h.core);
    let session = OperatorSession::new(core);
    let saved: IssueSaveResponse = serde_json::from_value(
        session
            .call(
                "issue.save",
                &save_payload(
                    0,
                    "acceptance-label",
                    json!({
                        "project_id": 1,
                        "title": "Label test",
                    }),
                ),
            )
            .expect("create succeeds"),
    )
    .unwrap();

    let error = session
        .call(
            "issue.save",
            &save_payload(
                1,
                "acceptance-label-add",
                json!({
                    "identifier": saved.identifier,
                    "add_labels": ["task"],
                }),
            ),
        )
        .expect_err("retired label refused");
    assert_eq!(error.code, kanban_dto::ErrorCode::InvalidRequest);
}

#[test]
fn acceptance_add_labels_persist_on_create_and_update() {
    let mut h = harness();
    let attachments = h._dir.path().join("attachments");
    std::fs::create_dir_all(&attachments).expect("attachments dir");
    register_issue_stack(&mut h.core, &h.database, &attachments);
    let core = Arc::new(h.core);
    let session = OperatorSession::new(core);

    let created: IssueSaveResponse = serde_json::from_value(
        session
            .call(
                "issue.save",
                &save_payload(
                    0,
                    "acceptance-label-create",
                    json!({
                        "project_id": 1,
                        "title": "Labelled on create",
                        "add_labels": ["task"],
                    }),
                ),
            )
            .expect("create with label succeeds"),
    )
    .unwrap();

    let read: IssueRecord = serde_json::from_value(
        session
            .call(
                "issue.get",
                &serde_json::to_value(IssueGetQuery {
                    identifier: created.identifier.clone(),
                })
                .unwrap(),
            )
            .expect("get succeeds"),
    )
    .unwrap();
    assert_eq!(read.labels, vec!["task".to_owned()]);

    session
        .call(
            "issue.save",
            &save_payload(
                1,
                "acceptance-label-switch-type",
                json!({
                    "identifier": created.identifier,
                    "add_labels": ["bug"],
                }),
            ),
        )
        .expect("switch type label succeeds");

    let relisted: IssueRecord = serde_json::from_value(
        session
            .call(
                "issue.get",
                &serde_json::to_value(IssueGetQuery {
                    identifier: created.identifier.clone(),
                })
                .unwrap(),
            )
            .expect("get after label switch succeeds"),
    )
    .unwrap();
    assert_eq!(relisted.labels, vec!["bug".to_owned()]);

    let listed: kanban_dto::IssueListResponse = serde_json::from_value(
        session
            .call("issue.list", &json!({}))
            .expect("list without project_id succeeds"),
    )
    .unwrap();
    let found = listed
        .issues
        .iter()
        .find(|issue| issue.identifier == created.identifier)
        .expect("issue appears in global list");
    assert_eq!(found.labels, vec!["bug".to_owned()]);
}

#[test]
fn acceptance_list_issues_without_project_id_spans_projects() {
    let mut h = harness();
    let attachments = h._dir.path().join("attachments");
    std::fs::create_dir_all(&attachments).expect("attachments dir");
    register_issue_stack(&mut h.core, &h.database, &attachments);

    let projects = SqliteProjectStore::new(&h.database);
    let edge = ProjectRegistration::new(
        "EDGE",
        "Edge tooling",
        "/repositories/edge",
        "/workspaces/edge.seed",
        "main",
        "edge.seed",
        Some("edge-main"),
        None,
    )
    .expect("edge registration validates");
    projects
        .create(&edge, &|id| {
            kanban_app::TimelineEnvelope::project(
                id.value(),
                kanban_dto::TimelineEventKind::Transition,
                Some(kanban_dto::TimelineEntityRef {
                    kind: kanban_dto::TimelineEntityKind::Project,
                    id: id.value().to_string(),
                }),
                json!({ "action": "registered" }),
            )
        })
        .expect("edge project lands");

    let core = Arc::new(h.core);
    let session = OperatorSession::new(core);

    let edge_save: IssueSaveResponse = serde_json::from_value(
        session
            .call(
                "issue.save",
                &save_payload(
                    0,
                    "acceptance-edge-issue",
                    json!({
                        "project_id": 2,
                        "title": "Edge issue",
                    }),
                ),
            )
            .expect("edge issue saves"),
    )
    .unwrap();
    assert_eq!(edge_save.identifier, "EDGE-1");

    let listed: kanban_dto::IssueListResponse = serde_json::from_value(
        session.call("issue.list", &json!({})).expect("global list succeeds"),
    )
    .unwrap();
    assert!(
        listed
            .issues
            .iter()
            .any(|issue| issue.identifier == edge_save.identifier),
        "global list includes the edge project issue"
    );
}

#[test]
fn acceptance_title_update_changes_read_back_and_version() {
    let mut h = harness();
    let attachments = h._dir.path().join("attachments");
    register_issue_stack(&mut h.core, &h.database, &attachments);
    let session = OperatorSession::new(Arc::new(h.core));

    let saved: IssueSaveResponse = serde_json::from_value(
        session
            .call(
                "issue.save",
                &save_payload(
                    0,
                    "acceptance-title",
                    json!({ "project_id": 1, "title": "Original" }),
                ),
            )
            .unwrap(),
    )
    .unwrap();

    session
        .call(
            "issue.save",
            &save_payload(
                1,
                "acceptance-title-rename",
                json!({ "identifier": saved.identifier, "title": "Renamed" }),
            ),
        )
        .unwrap();

    let read: IssueRecord = serde_json::from_value(
        session
            .call(
                "issue.get",
                &serde_json::to_value(IssueGetQuery {
                    identifier: saved.identifier,
                })
                .unwrap(),
            )
            .unwrap(),
    )
    .unwrap();
    assert_eq!(read.title, "Renamed");
    assert!(read.version >= 2);
}

#[test]
fn acceptance_state_history_lists_each_move() {
    let mut h = harness();
    let attachments = h._dir.path().join("attachments");
    register_issue_stack(&mut h.core, &h.database, &attachments);
    let session = OperatorSession::new(Arc::new(h.core));

    let saved: IssueSaveResponse = serde_json::from_value(
        session
            .call(
                "issue.save",
                &save_payload(
                    0,
                    "acceptance-history",
                    json!({ "project_id": 1, "title": "History" }),
                ),
            )
            .unwrap(),
    )
    .unwrap();
    session
        .call(
            "issue.save",
            &save_payload(
                1,
                "acceptance-history-active",
                json!({ "identifier": saved.identifier, "state": "Active" }),
            ),
        )
        .unwrap();

    let read: IssueRecord = serde_json::from_value(
        session
            .call(
                "issue.get",
                &serde_json::to_value(IssueGetQuery {
                    identifier: saved.identifier,
                })
                .unwrap(),
            )
            .unwrap(),
    )
    .unwrap();
    assert!(!read.state_history.is_empty());
}

#[test]
fn acceptance_cross_project_milestone_is_rejected() {
    let mut h = harness();
    let attachments = h._dir.path().join("attachments");
    register_issue_stack(&mut h.core, &h.database, &attachments);
    let projects = SqliteProjectStore::new(&h.database);
    let edge = ProjectRegistration::new(
        "EDGE",
        "Edge tooling",
        "/repositories/edge",
        "/workspaces/edge.seed",
        "main",
        "edge.seed",
        Some("edge-main"),
        None,
    )
    .unwrap();
    projects
        .create(&edge, &|id| {
            kanban_app::TimelineEnvelope::project(
                id.value(),
                kanban_dto::TimelineEventKind::Transition,
                Some(kanban_dto::TimelineEntityRef {
                    kind: kanban_dto::TimelineEntityKind::Project,
                    id: id.value().to_string(),
                }),
                json!({ "action": "registered" }),
            )
        })
        .unwrap();
    let conn = rusqlite::Connection::open(&h.database_path).unwrap();
    conn.execute(
        "INSERT INTO issue_milestones (project_id, name, sort_order) VALUES (2, 'Edge', 0)",
        [],
    )
    .unwrap();
    let milestone_id: i64 = conn
        .query_row(
            "SELECT id FROM issue_milestones WHERE name = 'Edge'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    drop(conn);

    let session = OperatorSession::new(Arc::new(h.core));
    let saved: IssueSaveResponse = serde_json::from_value(
        session
            .call(
                "issue.save",
                &save_payload(
                    0,
                    "acceptance-milestone-target",
                    json!({ "project_id": 1, "title": "Milestone test" }),
                ),
            )
            .unwrap(),
    )
    .unwrap();
    let error = session
        .call(
            "issue.save",
            &save_payload(
                1,
                "acceptance-milestone-bad",
                json!({
                    "identifier": saved.identifier,
                    "milestone_id": milestone_id,
                }),
            ),
        )
        .expect_err("cross-project milestone refused");
    assert_eq!(error.code, kanban_dto::ErrorCode::InvalidRequest);
}

#[test]
fn acceptance_comment_tools_round_trip() {
    let mut h = harness();
    let attachments = h._dir.path().join("attachments");
    register_issue_stack(&mut h.core, &h.database, &attachments);
    let session = OperatorSession::new(Arc::new(h.core));
    let saved: IssueSaveResponse = serde_json::from_value(
        session
            .call(
                "issue.save",
                &save_payload(
                    0,
                    "acceptance-comment-issue",
                    json!({ "project_id": 1, "title": "Comment host" }),
                ),
            )
            .unwrap(),
    )
    .unwrap();

    let created = session
        .call(
            "comment.create",
            &json!({
                "mutation": mutation(0, "acceptance-comment"),
                "project_id": 1,
                "target": { "kind": "ticket", "id": saved.ticket_id.to_string() },
                "text": "Ship report",
            }),
        )
        .unwrap();
    let comment_id = created["id"].as_u64().expect("comment id");

    session
        .call(
            "comment.edit",
            &json!({
                "mutation": mutation(1, "acceptance-comment-edit"),
                "comment_id": comment_id,
                "text": "Ship report v2",
            }),
        )
        .unwrap();

    let revisions = session
        .call("comment.revisions", &json!({ "comment_id": comment_id }))
        .unwrap();
    assert_eq!(revisions["revisions"].as_array().unwrap().len(), 2);
}

#[test]
fn acceptance_unknown_issue_key_is_not_found() {
    let mut h = harness();
    let attachments = h._dir.path().join("attachments");
    register_issue_stack(&mut h.core, &h.database, &attachments);
    let session = OperatorSession::new(Arc::new(h.core));
    let error = session
        .call(
            "issue.get",
            &serde_json::to_value(IssueGetQuery {
                identifier: "CORE-404".into(),
            })
            .unwrap(),
        )
        .expect_err("missing issue");
    assert_eq!(error.code, kanban_dto::ErrorCode::NotFound);
}

#[test]
fn acceptance_sub_issue_links_parent_and_lists_children() {
    let mut h = harness();
    let attachments = h._dir.path().join("attachments");
    register_issue_stack(&mut h.core, &h.database, &attachments);
    let session = OperatorSession::new(Arc::new(h.core));

    let parent: IssueSaveResponse = serde_json::from_value(
        session
            .call(
                "issue.save",
                &save_payload(
                    0,
                    "acceptance-parent",
                    json!({ "project_id": 1, "title": "Parent" }),
                ),
            )
            .unwrap(),
    )
    .unwrap();

    let child: IssueSaveResponse = serde_json::from_value(
        session
            .call(
                "issue.save",
                &save_payload(
                    0,
                    "acceptance-child",
                    json!({
                        "project_id": 1,
                        "title": "Child",
                        "parent_id": parent.ticket_id,
                    }),
                ),
            )
            .unwrap(),
    )
    .unwrap();

    let read_parent: IssueRecord = serde_json::from_value(
        session
            .call(
                "issue.get",
                &serde_json::to_value(IssueGetQuery {
                    identifier: parent.identifier.clone(),
                })
                .unwrap(),
            )
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        read_parent.children.as_ref().unwrap()[0].identifier,
        child.identifier
    );

    let read_child: IssueRecord = serde_json::from_value(
        session
            .call(
                "issue.get",
                &serde_json::to_value(IssueGetQuery {
                    identifier: child.identifier,
                })
                .unwrap(),
            )
            .unwrap(),
    )
    .unwrap();
    assert_eq!(read_child.parent.as_deref(), Some(parent.identifier.as_str()));
}
