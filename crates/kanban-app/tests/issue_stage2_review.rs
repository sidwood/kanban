//! Stage 2 review regressions (operator parity).

use std::sync::Arc;

use kanban_app::{
    CommentIssueTargets, CommentStore, Core, IssueStore, IssueStoreCommentTargets, OperatorSession,
};
use kanban_dto::{ErrorCode, IssueGetQuery, IssueListResponse, IssueRecord, IssueSaveResponse};
use kanban_storage::SqliteCommentStore;
use kanban_storage::{
    Database, SqliteIssueStore, SqlitePlanStore, SqliteProjectStore, SqliteSpecStore,
    SqliteTicketStore,
};
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

struct Stage2Harness {
    _dir: tempfile::TempDir,
    session: OperatorSession,
    database_path: std::path::PathBuf,
}

fn stage2_harness() -> Stage2Harness {
    let mut h = harness();
    let attachments = h._dir.path().join("attachments");
    register_issue_stack(&mut h.core, &h.database, &attachments);
    Stage2Harness {
        session: OperatorSession::new(Arc::new(h.core)),
        database_path: h.database_path,
        _dir: h._dir,
    }
}

fn session_with_path() -> (OperatorSession, std::path::PathBuf, tempfile::TempDir) {
    let held = stage2_harness();
    (held.session, held.database_path, held._dir)
}

fn issue_version(session: &OperatorSession, identifier: &str) -> u64 {
    let read: IssueRecord = serde_json::from_value(
        session
            .call(
                "issue.get",
                &serde_json::to_value(IssueGetQuery {
                    identifier: identifier.to_owned(),
                })
                .unwrap(),
            )
            .unwrap(),
    )
    .unwrap();
    read.version
}

fn save_issue(
    session: &OperatorSession,
    version: u64,
    key: &str,
    fields: serde_json::Value,
) -> IssueSaveResponse {
    serde_json::from_value(
        session
            .call("issue.save", &save_payload(version, key, fields))
            .expect("issue.save succeeds"),
    )
    .expect("save response shape")
}

#[test]
fn list_issues_cursor_pages_without_skipping_rows() {
    let (session, _, _dir) = session_with_path();
    let first = save_issue(
        &session,
        0,
        "cursor-a",
        json!({ "project_id": 1, "title": "Alpha" }),
    );
    let second = save_issue(
        &session,
        0,
        "cursor-b",
        json!({ "project_id": 1, "title": "Bravo" }),
    );
    let third = save_issue(
        &session,
        0,
        "cursor-c",
        json!({ "project_id": 1, "title": "Charlie" }),
    );

    let page1: IssueListResponse = serde_json::from_value(
        session
            .call("issue.list", &json!({ "project_id": 1, "limit": 1 }))
            .unwrap(),
    )
    .unwrap();
    assert_eq!(page1.issues.len(), 1);
    assert_eq!(page1.issues[0].ticket_id, third.ticket_id);
    assert!(
        page1.next_cursor.as_deref().is_some_and(|cursor| {
            cursor
                .rsplit_once('/')
                .is_some_and(|(_, id)| id == third.ticket_id.to_string())
        }),
        "the issued cursor keeps the last row as its boundary"
    );

    let page2: IssueListResponse = serde_json::from_value(
        session
            .call(
                "issue.list",
                &json!({
                    "project_id": 1,
                    "limit": 1,
                    "cursor": page1.next_cursor,
                }),
            )
            .unwrap(),
    )
    .unwrap();
    assert_eq!(page2.issues.len(), 1);
    assert_eq!(page2.issues[0].ticket_id, second.ticket_id);

    let page3: IssueListResponse = serde_json::from_value(
        session
            .call(
                "issue.list",
                &json!({
                    "project_id": 1,
                    "limit": 1,
                    "cursor": page2.next_cursor,
                }),
            )
            .unwrap(),
    )
    .unwrap();
    assert_eq!(page3.issues.len(), 1);
    assert_eq!(page3.issues[0].ticket_id, first.ticket_id);
    assert!(page3.next_cursor.is_none());

    let _ = (first, second);
}

#[test]
fn list_issues_rejects_unknown_cursor() {
    let (session, _, _dir) = session_with_path();
    save_issue(
        &session,
        0,
        "cursor-host",
        json!({ "project_id": 1, "title": "Host" }),
    );
    let error = session
        .call(
            "issue.list",
            &json!({ "project_id": 1, "cursor": "999999" }),
        )
        .expect_err("unknown cursor refused");
    assert_eq!(error.code, ErrorCode::InvalidRequest);
}

#[test]
fn duplicate_merge_moves_state_relations_and_metadata() {
    let (session, _, _dir) = session_with_path();
    let canonical = save_issue(
        &session,
        0,
        "dup-canonical",
        json!({ "project_id": 1, "title": "Canonical" }),
    );
    let other = save_issue(
        &session,
        0,
        "dup-other",
        json!({ "project_id": 1, "title": "Other" }),
    );
    let copy = save_issue(
        &session,
        0,
        "dup-copy",
        json!({
            "project_id": 1,
            "title": "Copy",
            "add_blocks": [other.identifier.clone()],
            "add_related": [other.identifier.clone()],
        }),
    );
    save_issue(
        &session,
        issue_version(&session, &copy.identifier),
        "dup-mark",
        json!({
            "identifier": copy.identifier,
            "duplicate_of": canonical.identifier,
        }),
    );

    let read_copy: IssueRecord = serde_json::from_value(
        session
            .call(
                "issue.get",
                &serde_json::to_value(IssueGetQuery {
                    identifier: copy.identifier.clone(),
                })
                .unwrap(),
            )
            .unwrap(),
    )
    .unwrap();
    assert_eq!(read_copy.state, "Duplicate");
    assert_eq!(
        read_copy.duplicate_of.as_deref(),
        Some(canonical.identifier.as_str())
    );
    assert!(
        read_copy
            .relations
            .iter()
            .all(|relation| relation.identifier != canonical.identifier),
        "outbound relations should move off the duplicate"
    );

    let read_canonical: IssueRecord = serde_json::from_value(
        session
            .call(
                "issue.get",
                &serde_json::to_value(IssueGetQuery {
                    identifier: canonical.identifier.clone(),
                })
                .unwrap(),
            )
            .unwrap(),
    )
    .unwrap();
    assert!(
        read_canonical
            .relations
            .iter()
            .any(|relation| relation.kind == "blocks" && relation.identifier == other.identifier),
        "blocks edge should move to the canonical issue"
    );
    assert!(
        read_canonical
            .relations
            .iter()
            .any(|relation| relation.kind == "related" && relation.identifier == other.identifier),
        "related edge should move to the canonical issue"
    );
    assert!(
        read_canonical
            .duplicates
            .as_ref()
            .is_some_and(|rows| rows.iter().any(|row| row == &copy.identifier)),
        "canonical lists the duplicate identifier"
    );
}

#[test]
fn related_links_are_visible_from_both_endpoints() {
    let (session, _, _dir) = session_with_path();
    let left = save_issue(
        &session,
        0,
        "related-left",
        json!({ "project_id": 1, "title": "Left" }),
    );
    let right = save_issue(
        &session,
        0,
        "related-right",
        json!({
            "project_id": 1,
            "title": "Right",
            "add_related": [left.identifier.clone()],
        }),
    );

    let read_left: IssueRecord = serde_json::from_value(
        session
            .call(
                "issue.get",
                &serde_json::to_value(IssueGetQuery {
                    identifier: left.identifier,
                })
                .unwrap(),
            )
            .unwrap(),
    )
    .unwrap();
    let read_right: IssueRecord = serde_json::from_value(
        session
            .call(
                "issue.get",
                &serde_json::to_value(IssueGetQuery {
                    identifier: right.identifier.clone(),
                })
                .unwrap(),
            )
            .unwrap(),
    )
    .unwrap();

    assert!(
        read_right
            .relations
            .iter()
            .any(|relation| relation.kind == "related"
                && relation.identifier == read_left.identifier)
    );
    assert!(
        read_left
            .relations
            .iter()
            .any(|relation| relation.kind == "related"
                && relation.identifier == read_right.identifier)
    );

    save_issue(
        &session,
        issue_version(&session, &read_right.identifier),
        "related-remove",
        json!({
            "identifier": read_right.identifier,
            "remove_related": [read_left.identifier],
        }),
    );
    let after_remove: IssueRecord = serde_json::from_value(
        session
            .call(
                "issue.get",
                &serde_json::to_value(IssueGetQuery {
                    identifier: right.identifier,
                })
                .unwrap(),
            )
            .unwrap(),
    )
    .unwrap();
    assert!(
        after_remove
            .relations
            .iter()
            .all(|relation| relation.kind != "related")
    );
}

#[test]
fn comment_list_by_issue_includes_author_and_timestamps() {
    let (session, _, _dir) = session_with_path();
    let host = save_issue(
        &session,
        0,
        "comment-host",
        json!({ "project_id": 1, "title": "Comment host" }),
    );
    session
        .call(
            "comment.create",
            &json!({
                "mutation": mutation(0, "stage2-comment"),
                "project_id": 1,
                "target": { "kind": "ticket", "id": host.ticket_id.to_string() },
                "text": "Note",
                "author": "reviewer",
            }),
        )
        .unwrap();

    let listed = session
        .call(
            "comment.list",
            &json!({
                "target": { "kind": "ticket", "id": host.ticket_id.to_string() },
                "limit": 10,
            }),
        )
        .unwrap();
    let comments = listed["comments"].as_array().unwrap();
    assert_eq!(comments.len(), 1);
    assert_eq!(comments[0]["author"], "reviewer");
    assert!(comments[0]["created_at"].is_string());
    assert_eq!(comments[0]["body"], "Note");
}

#[test]
fn sub_issue_sort_order_is_respected_when_listing_children() {
    let (session, _, _dir) = session_with_path();
    let parent = save_issue(
        &session,
        0,
        "sort-parent",
        json!({ "project_id": 1, "title": "Parent" }),
    );
    let first = save_issue(
        &session,
        0,
        "sort-first",
        json!({
            "project_id": 1,
            "title": "First child",
            "parent_id": parent.ticket_id,
            "sort_order": 2.0,
        }),
    );
    let second = save_issue(
        &session,
        0,
        "sort-second",
        json!({
            "project_id": 1,
            "title": "Second child",
            "parent_id": parent.ticket_id,
            "sort_order": 1.0,
        }),
    );

    let read: IssueRecord = serde_json::from_value(
        session
            .call(
                "issue.get",
                &serde_json::to_value(IssueGetQuery {
                    identifier: parent.identifier,
                })
                .unwrap(),
            )
            .unwrap(),
    )
    .unwrap();
    let children = read.children.expect("children listed");
    assert_eq!(children.len(), 2);
    assert_eq!(children[0].identifier, second.identifier);
    assert_eq!(children[1].identifier, first.identifier);
}

fn enable_auto_close(session: &OperatorSession, project_id: u64) {
    session
        .call(
            "issue.update_project_settings",
            &json!({
                "mutation": mutation(0, "enable-auto-close"),
                "project_id": project_id,
                "auto_close_sub_issues": true,
            }),
        )
        .expect("enable auto-close");
}

#[test]
fn auto_close_parent_when_all_sub_issues_complete() {
    let (session, database_path, _dir) = session_with_path();
    enable_auto_close(&session, 1);
    let parent = save_issue(
        &session,
        0,
        "auto-parent",
        json!({ "project_id": 1, "title": "Parent", "state": "Active" }),
    );
    let child = save_issue(
        &session,
        0,
        "auto-child",
        json!({
            "project_id": 1,
            "title": "Child",
            "parent_id": parent.ticket_id,
            "state": "Active",
        }),
    );
    save_issue(
        &session,
        issue_version(&session, &child.identifier),
        "auto-child-done",
        json!({
            "identifier": child.identifier,
            "state": "Done",
        }),
    );

    let read: IssueRecord = serde_json::from_value(
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
    assert_eq!(read.state, "Done");

    let conn = rusqlite::Connection::open(&database_path).unwrap();
    let history_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM issue_state_history
             WHERE ticket_id = ?1 AND workflow_state_id IN (
                 SELECT id FROM issue_workflow_states WHERE name = 'Done'
             )",
            [parent.ticket_id as i64],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        history_count, 1,
        "auto-close should not duplicate Done history"
    );
}

#[test]
fn auto_close_skips_parent_already_terminal() {
    let (session, database_path, _dir) = session_with_path();
    enable_auto_close(&session, 1);
    let parent = save_issue(
        &session,
        0,
        "auto-closed-parent",
        json!({ "project_id": 1, "title": "Parent", "state": "Done" }),
    );
    let child = save_issue(
        &session,
        0,
        "auto-closed-child",
        json!({
            "project_id": 1,
            "title": "Child",
            "parent_id": parent.ticket_id,
            "state": "Active",
        }),
    );
    save_issue(
        &session,
        issue_version(&session, &child.identifier),
        "auto-closed-child-done",
        json!({
            "identifier": child.identifier,
            "state": "Done",
        }),
    );

    let read: IssueRecord = serde_json::from_value(
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
    assert_eq!(read.state, "Done");

    let conn = rusqlite::Connection::open(&database_path).unwrap();
    let history_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM issue_state_history
             WHERE ticket_id = ?1 AND workflow_state_id IN (
                 SELECT id FROM issue_workflow_states WHERE name = 'Done'
             )",
            [parent.ticket_id as i64],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(history_count, 1);
}

#[test]
fn null_clears_due_milestone_and_parent() {
    let (session, database_path, _dir) = session_with_path();
    let conn = rusqlite::Connection::open(&database_path).unwrap();
    conn.execute(
        "INSERT INTO issue_milestones (project_id, name, sort_order) VALUES (1, 'M1', 0)",
        [],
    )
    .unwrap();
    let milestone_id: i64 = conn
        .query_row(
            "SELECT id FROM issue_milestones WHERE name = 'M1'",
            [],
            |row| row.get(0),
        )
        .unwrap();

    let parent = save_issue(
        &session,
        0,
        "null-parent",
        json!({ "project_id": 1, "title": "Parent" }),
    );
    let mut host = save_issue(
        &session,
        0,
        "null-host",
        json!({
            "project_id": 1,
            "title": "Host",
            "parent_id": parent.ticket_id,
            "due_date": "2030-01-01",
            "milestone_id": milestone_id,
        }),
    );

    host = save_issue(
        &session,
        issue_version(&session, &host.identifier),
        "null-clear",
        json!({
            "identifier": host.identifier,
            "due_date": null,
            "milestone_id": null,
            "parent_id": null,
        }),
    );

    let read: IssueRecord = serde_json::from_value(
        session
            .call(
                "issue.get",
                &serde_json::to_value(IssueGetQuery {
                    identifier: host.identifier,
                })
                .unwrap(),
            )
            .unwrap(),
    )
    .unwrap();
    assert!(read.due_date.is_none());
    assert!(read.milestone_id.is_none());
    assert!(read.parent.is_none());
}

#[test]
fn get_issue_state_history_records_each_transition() {
    let (session, _, _dir) = session_with_path();
    let saved = save_issue(
        &session,
        0,
        "history-host",
        json!({ "project_id": 1, "title": "History" }),
    );
    save_issue(
        &session,
        1,
        "history-active",
        json!({ "identifier": saved.identifier, "state": "Active" }),
    );
    let updated: IssueRecord = serde_json::from_value(
        session
            .call(
                "issue.get",
                &serde_json::to_value(IssueGetQuery {
                    identifier: saved.identifier.clone(),
                })
                .unwrap(),
            )
            .unwrap(),
    )
    .unwrap();
    save_issue(
        &session,
        updated.version,
        "history-done",
        json!({ "identifier": saved.identifier, "state": "Done" }),
    );

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
    let names: Vec<_> = read
        .state_history
        .iter()
        .map(|row| row.state.as_str())
        .collect();
    assert_eq!(names, ["Draft", "Active", "Done"]);
    assert!(read.state_history[0].ended_at.is_some());
    assert!(read.state_history[1].ended_at.is_some());
    assert!(read.state_history[2].ended_at.is_none());
}
