//! Lin MCP acceptance (L1–L10) on fresh DB and migration fixture paths.

use std::sync::Arc;

use kanban_app::{
    CommentIssueTargets, CommentStore, Core, IssueStore, IssueStoreCommentTargets, OperatorSession,
};
use kanban_dto::{ErrorCode, IssueRecord, IssueSaveResponse};
use kanban_storage::SqliteCommentStore;
use kanban_storage::{
    AllowAllMigrations, Database, SqliteIssueStore, SqlitePlanStore, SqliteProjectStore,
    SqliteSpecStore, SqliteTicketStore, migrations,
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

fn session() -> OperatorSession {
    let mut h = harness();
    let attachments = h._dir.path().join("attachments");
    register_issue_stack(&mut h.core, &h.database, &attachments);
    OperatorSession::new(Arc::new(h.core))
}

fn migrated_session() -> (tempfile::TempDir, OperatorSession) {
    let dir = tempfile::TempDir::new().expect("scratch dir");
    let path = dir.path().join("kanban.sqlite");
    let mut database = Database::open(&path).expect("database opens");
    migrations::apply_through(&database.connection(), 52).expect("schema 52");
    common::seed_project_profile(&database);
    let ticket_id = common::insert_ready_ticket(&path, 1, "normal");
    common::insert_blocker(&path, ticket_id);
    database
        .migrate(&AllowAllMigrations)
        .expect("migrate to head");
    let attachments = dir.path().join("attachments");
    let idempotency = Arc::new(kanban_storage::SqliteIdempotencyStore::new(
        &database,
        kanban_storage::RetentionPolicy::keep_most_recent(
            std::num::NonZeroU32::new(100).expect("non-zero"),
        ),
    ));
    let mut core = kanban_app::dispatch::Core::new(
        kanban_app::catalog::exposed_operations(),
        idempotency,
        Arc::new(kanban_app::events::NoopEventSink),
    );
    register_issue_stack(&mut core, &database, &attachments);
    (dir, OperatorSession::new(Arc::new(core)))
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
            .expect("issue.save"),
    )
    .expect("save shape")
}

fn issue_version(session: &OperatorSession, identifier: &str) -> u64 {
    let read: IssueRecord = serde_json::from_value(
        session
            .call("issue.get", &json!({ "identifier": identifier }))
            .unwrap(),
    )
    .unwrap();
    read.version
}

#[test]
fn l1_null_clears_distinguish_absent_from_null() {
    let session = session();
    let host = save_issue(
        &session,
        0,
        "l1-host",
        json!({ "project_id": 1, "title": "Host", "due_date": "2030-06-01" }),
    );
    save_issue(
        &session,
        2,
        "l1-clear",
        json!({ "identifier": host.identifier, "due_date": null }),
    );
    let read: IssueRecord = serde_json::from_value(
        session
            .call("issue.get", &json!({ "identifier": host.identifier }))
            .unwrap(),
    )
    .unwrap();
    assert!(read.due_date.is_none());
}

#[test]
fn l2_auto_close_default_off_until_project_setting_enabled() {
    let session = session();
    let parent = save_issue(
        &session,
        0,
        "l2-parent",
        json!({ "project_id": 1, "title": "Parent", "state": "Active" }),
    );
    let child = save_issue(
        &session,
        0,
        "l2-child",
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
        "l2-child-done",
        json!({ "identifier": child.identifier, "state": "Done" }),
    );
    let read: IssueRecord = serde_json::from_value(
        session
            .call("issue.get", &json!({ "identifier": parent.identifier }))
            .unwrap(),
    )
    .unwrap();
    assert_eq!(read.state, "Active", "auto-close is off by default");
}

#[test]
fn l3_get_issue_returns_lifecycle_timestamps() {
    let session = session();
    let saved = save_issue(
        &session,
        0,
        "l3-ts",
        json!({ "project_id": 1, "title": "Timestamps", "state": "Active" }),
    );
    save_issue(
        &session,
        2,
        "l3-done",
        json!({ "identifier": saved.identifier, "state": "Done" }),
    );
    let read: IssueRecord = serde_json::from_value(
        session
            .call("issue.get", &json!({ "identifier": saved.identifier }))
            .unwrap(),
    )
    .unwrap();
    assert!(read.started_at.is_some());
    assert!(read.completed_at.is_some());
    save_issue(
        &session,
        read.version,
        "l3-reopen",
        json!({ "identifier": saved.identifier, "state": "Active" }),
    );
    let reopened: IssueRecord = serde_json::from_value(
        session
            .call("issue.get", &json!({ "identifier": saved.identifier }))
            .unwrap(),
    )
    .unwrap();
    assert!(reopened.completed_at.is_none());
}

#[test]
fn l4_parent_key_and_sub_issue_counts() {
    let session = session();
    let parent = save_issue(
        &session,
        0,
        "l4-parent",
        json!({ "project_id": 1, "title": "Parent" }),
    );
    save_issue(
        &session,
        0,
        "l4-child",
        json!({
            "project_id": 1,
            "title": "Child",
            "parent_id": parent.identifier,
            "state": "Done",
        }),
    );
    let read: IssueRecord = serde_json::from_value(
        session
            .call("issue.get", &json!({ "identifier": parent.identifier }))
            .unwrap(),
    )
    .unwrap();
    let progress = read.sub_issues.expect("sub-issue progress");
    assert_eq!(progress.total, 1);
    assert_eq!(progress.done, 1);
}

#[test]
fn l5_blocked_by_and_description_mention_related() {
    let session = session();
    let blocker = save_issue(
        &session,
        0,
        "l5-blocker",
        json!({ "project_id": 1, "title": "Blocker" }),
    );
    let host = save_issue(
        &session,
        0,
        "l5-host",
        json!({ "project_id": 1, "title": "Host" }),
    );
    save_issue(
        &session,
        issue_version(&session, &host.identifier),
        "l5-block",
        json!({
            "identifier": host.identifier,
            "add_blocked_by": [blocker.identifier],
        }),
    );
    save_issue(
        &session,
        issue_version(&session, &host.identifier),
        "l5-mention",
        json!({
            "identifier": host.identifier,
            "description": format!("See {} for context.", blocker.identifier),
        }),
    );
    let read: IssueRecord = serde_json::from_value(
        session
            .call("issue.get", &json!({ "identifier": host.identifier }))
            .unwrap(),
    )
    .unwrap();
    assert!(
        read.relations
            .iter()
            .any(|r| r.kind == "blocked-by" && r.identifier == blocker.identifier)
    );
    assert!(
        read.relations
            .iter()
            .any(|r| r.kind == "related" && r.identifier == blocker.identifier)
    );
}

#[test]
fn l6_comment_body_thread_and_resolve() {
    let session = session();
    let host = save_issue(
        &session,
        0,
        "l6-host",
        json!({ "project_id": 1, "title": "Host" }),
    );
    let body = "Report line\n";
    let created = session
        .call(
            "comment.create",
            &json!({
                "mutation": mutation(0, "l6-root"),
                "project_id": 1,
                "target": { "kind": "ticket", "id": host.ticket_id.to_string() },
                "text": body,
                "author": "lin",
            }),
        )
        .unwrap();
    let root_id = created["id"].as_u64().unwrap();
    let root_version = created["version"].as_u64().unwrap();
    session
        .call(
            "comment.create",
            &json!({
                "mutation": mutation(0, "l6-reply"),
                "project_id": 1,
                "target": { "kind": "ticket", "id": host.ticket_id.to_string() },
                "text": "Reply",
                "author": "lin",
                "parent_comment_id": root_id,
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
    assert_eq!(comments[0]["body"], body);
    assert_eq!(comments[1]["parent_comment_id"], root_id);
    session
        .call(
            "comment.resolve",
            &json!({
                "mutation": mutation(root_version, "l6-resolve"),
                "comment_id": root_id,
                "resolved": true,
            }),
        )
        .unwrap();
    let resolved = session
        .call(
            "comment.list",
            &json!({
                "target": { "kind": "ticket", "id": host.ticket_id.to_string() },
                "limit": 10,
            }),
        )
        .unwrap();
    assert!(resolved["comments"][0]["resolved_at"].is_string());
}

#[test]
fn l7_migrated_database_preserves_blocker_prose_on_issue_get() {
    let (_dir, session) = migrated_session();
    let read: IssueRecord = serde_json::from_value(
        session
            .call("issue.get", &json!({ "identifier": "CORE-1" }))
            .expect("issue.get on migrated row"),
    )
    .expect("issue shape");
    assert!(
        read.description
            .contains("waiting on an unregistered vendor"),
        "legacy ticket_blockers prose should appear on the migrated issue"
    );
}

#[test]
fn l8_errors_are_specific_not_opaque() {
    let session = session();
    let error = session
        .call("issue.get", &json!({ "identifier": "NOPE-999" }))
        .expect_err("unknown issue");
    assert_eq!(error.code, ErrorCode::NotFound);

    let host = save_issue(
        &session,
        0,
        "l8-host",
        json!({ "project_id": 1, "title": "Host" }),
    );
    let bad = session
        .call(
            "issue.save",
            &save_payload(
                issue_version(&session, &host.identifier),
                "l8-priority",
                json!({ "identifier": host.identifier, "priority": 9 }),
            ),
        )
        .expect_err("bad priority");
    assert_eq!(bad.code, ErrorCode::InvalidRequest);

    let case = session
        .call(
            "issue.get",
            &json!({ "identifier": host.identifier.to_lowercase() }),
        )
        .expect("case insensitive key");
    assert_eq!(case["identifier"], host.identifier);
}

#[test]
fn l9_list_filters_and_lookups() {
    let session = session();
    let listed = session
        .call("issue.list_projects", &json!({}))
        .expect("list projects");
    assert!(!listed["projects"].as_array().unwrap().is_empty());

    let labels = session
        .call("issue.list_labels", &json!({ "project_id": 1 }))
        .unwrap();
    assert!(labels["labels"].is_array());

    session
        .call(
            "issue.create_label",
            &json!({
                "mutation": mutation(0, "l9-label"),
                "project_id": 1,
                "name": "LinAccept",
                "color": "#336699",
                "group": "Type",
                "group_single_select": true,
            }),
        )
        .unwrap();

    session
        .call(
            "issue.create_milestone",
            &json!({
                "mutation": mutation(0, "l9-milestone"),
                "project_id": 1,
                "name": "Lin M1",
                "target_date": "2026-12-31",
            }),
        )
        .unwrap();

    let milestones = session
        .call("issue.list_milestones", &json!({ "project_id": 1 }))
        .unwrap();
    let row = milestones["milestones"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["name"] == "Lin M1")
        .expect("milestone listed");
    assert!(row["issues_total"].is_number());
}

#[test]
fn l10_description_patch_in_place_and_mcp_doc_matches_catalog() {
    let session = session();
    let host = save_issue(
        &session,
        0,
        "l10-host",
        json!({
            "project_id": 1,
            "title": "Patch",
            "description": "alpha beta gamma",
        }),
    );
    save_issue(
        &session,
        issue_version(&session, &host.identifier),
        "l10-patch",
        json!({
            "identifier": host.identifier,
            "description_patch": json!({ "find": "beta", "replace": "BETA" }).to_string(),
        }),
    );
    let read: IssueRecord = serde_json::from_value(
        session
            .call("issue.get", &json!({ "identifier": host.identifier }))
            .unwrap(),
    )
    .unwrap();
    assert_eq!(read.description, "alpha BETA gamma");

    let doc = std::fs::read_to_string(format!(
        "{}/../../docs/mcp-operator.md",
        env!("CARGO_MANIFEST_DIR")
    ))
    .expect("doc");
    for (core, mcp) in [
        ("issue.save", "save_issue"),
        ("issue.get", "get_issue"),
        ("issue.list", "list_issues"),
        ("issue.list_workflow_states", "list_issue_statuses"),
        ("issue.list_labels", "list_issue_labels"),
        ("issue.list_milestones", "list_milestones"),
        ("comment.create", "comment_create"),
        ("comment.edit", "comment_edit"),
        ("comment.revisions", "comment_revisions"),
        ("comment.list", "list_comments"),
    ] {
        assert!(
            doc.contains(mcp),
            "mcp-operator.md must name MCP tool `{mcp}` for `{core}`"
        );
    }
}
#[test]
fn comments_address_one_issue_by_key_or_id() {
    let session = session();
    let host = save_issue(
        &session,
        0,
        "stream-host",
        json!({ "project_id": 1, "title": "Stream host" }),
    );

    let by_key = session
        .call(
            "comment.create",
            &json!({
                "mutation": mutation(0, "stream-by-key"),
                "project_id": 1,
                "target": { "kind": "ticket", "id": host.identifier },
                "text": "Posted by key",
                "author": "lin",
            }),
        )
        .expect("the key-form create applies");
    session
        .call(
            "comment.create",
            &json!({
                "mutation": mutation(0, "stream-by-id"),
                "project_id": 1,
                "target": { "kind": "ticket", "id": host.ticket_id.to_string() },
                "text": "Posted by id",
                "author": "lin",
            }),
        )
        .expect("the id-form create applies");

    let by_id = session
        .call(
            "comment.list",
            &json!({
                "target": { "kind": "ticket", "id": host.ticket_id.to_string() },
                "limit": 10,
            }),
        )
        .expect("listing by id serves");
    let by_key_list = session
        .call(
            "comment.list",
            &json!({
                "target": { "kind": "ticket", "id": host.identifier },
                "limit": 10,
            }),
        )
        .expect("listing by key serves");

    let bodies: Vec<&str> = by_id["comments"]
        .as_array()
        .expect("an array of comments")
        .iter()
        .map(|comment| comment["body"].as_str().expect("body text"))
        .collect();
    assert_eq!(
        bodies,
        vec!["Posted by key", "Posted by id"],
        "both address forms read and write one stream"
    );
    assert_eq!(
        by_key_list["comments"], by_id["comments"],
        "the key and the id list the same stream"
    );
    assert_eq!(
        by_key["target"]["id"],
        json!(host.ticket_id.to_string()),
        "a created comment stores the issue id, not the key it was addressed by"
    );
}

#[test]
fn comment_targets_must_name_an_existing_issue() {
    let session = session();
    let host = save_issue(
        &session,
        0,
        "unknown-target-host",
        json!({ "project_id": 1, "title": "Host" }),
    );

    for raw in ["CORE-999", "999"] {
        let error = session
            .call(
                "comment.create",
                &json!({
                    "mutation": mutation(0, &format!("unknown-{raw}")),
                    "project_id": 1,
                    "target": { "kind": "ticket", "id": raw },
                    "text": "Ghost",
                    "author": "lin",
                }),
            )
            .expect_err("an unknown target is refused");
        assert_eq!(error.code, ErrorCode::NotFound, "target {raw}");
    }

    let listed = session
        .call(
            "comment.list",
            &json!({
                "target": { "kind": "ticket", "id": host.ticket_id.to_string() },
                "limit": 10,
            }),
        )
        .unwrap();
    assert_eq!(
        listed["comments"].as_array().unwrap().len(),
        0,
        "the refused writes stored nothing"
    );
}

#[test]
fn legacy_key_form_comments_join_the_resolved_stream() {
    let mut h = harness();
    let attachments = h._dir.path().join("attachments");
    register_issue_stack(&mut h.core, &h.database, &attachments);
    let session = OperatorSession::new(Arc::new(h.core));
    let host = save_issue(
        &session,
        0,
        "legacy-host",
        json!({ "project_id": 1, "title": "Host" }),
    );
    h.database
        .connection()
        .execute(
            "INSERT INTO comments (id, project_id, entity_kind, entity_id, version, author)
             VALUES (901, '1', 'ticket', ?1, 1, 'legacy')",
            [&host.identifier],
        )
        .expect("the legacy row lands");
    h.database
        .connection()
        .execute(
            "INSERT INTO comment_revisions (comment_id, revision, text)
             VALUES (901, 1, 'Legacy by key')",
            [],
        )
        .expect("the legacy revision lands");
    let mixed_case = format!(
        "{}{}",
        host.identifier.chars().next().expect("the key opens"),
        host.identifier[1..].to_lowercase(),
    );
    h.database
        .connection()
        .execute(
            "INSERT INTO comments (id, project_id, entity_kind, entity_id, version, author)
             VALUES (902, '1', 'ticket', ?1, 1, 'legacy')",
            [&mixed_case],
        )
        .expect("the mixed-case legacy row lands");
    h.database
        .connection()
        .execute(
            "INSERT INTO comment_revisions (comment_id, revision, text)
             VALUES (902, 1, 'Legacy mixed case')",
            [],
        )
        .expect("the mixed-case revision lands");

    let by_id = session
        .call(
            "comment.list",
            &json!({
                "target": { "kind": "ticket", "id": host.ticket_id.to_string() },
                "limit": 10,
            }),
        )
        .unwrap();
    let by_key = session
        .call(
            "comment.list",
            &json!({ "target": { "kind": "ticket", "id": host.identifier }, "limit": 10 }),
        )
        .unwrap();
    let bodies: Vec<_> = by_id["comments"]
        .as_array()
        .unwrap()
        .iter()
        .map(|comment| comment["body"].clone())
        .collect();
    assert_eq!(
        bodies,
        vec![json!("Legacy by key"), json!("Legacy mixed case")],
        "every pre-change letter-case spelling of the key still lists"
    );
    assert_eq!(by_key["comments"], by_id["comments"]);

    let reply = session
        .call(
            "comment.create",
            &json!({
                "mutation": mutation(0, "legacy-reply"),
                "project_id": 1,
                "target": { "kind": "ticket", "id": host.ticket_id.to_string() },
                "text": "Reply to the legacy root",
                "author": "lin",
                "parent_comment_id": 901,
            }),
        )
        .expect("a legacy key-form root still accepts replies");
    assert_eq!(
        reply["target"]["id"],
        json!(host.ticket_id.to_string()),
        "the reply stores the canonical issue id"
    );
}

#[test]
fn reply_parent_must_belong_to_the_same_issue() {
    let session = session();
    let host = save_issue(
        &session,
        0,
        "reply-host",
        json!({ "project_id": 1, "title": "Host" }),
    );
    let other = save_issue(
        &session,
        0,
        "reply-other",
        json!({ "project_id": 1, "title": "Other" }),
    );
    let root = session
        .call(
            "comment.create",
            &json!({
                "mutation": mutation(0, "reply-root"),
                "project_id": 1,
                "target": { "kind": "ticket", "id": host.ticket_id.to_string() },
                "text": "Root",
                "author": "lin",
            }),
        )
        .unwrap();
    let root_id = root["id"].as_u64().unwrap();

    let error = session
        .call(
            "comment.create",
            &json!({
                "mutation": mutation(0, "reply-cross"),
                "project_id": 1,
                "target": { "kind": "ticket", "id": other.ticket_id.to_string() },
                "text": "Cross-issue reply",
                "author": "lin",
                "parent_comment_id": root_id,
            }),
        )
        .expect_err("a parent from another issue is refused");

    assert_eq!(error.code, ErrorCode::InvalidRequest);
    assert_eq!(error.field.as_deref(), Some("parent_comment_id"));
    let listed = session
        .call(
            "comment.list",
            &json!({
                "target": { "kind": "ticket", "id": other.ticket_id.to_string() },
                "limit": 10,
            }),
        )
        .unwrap();
    assert_eq!(
        listed["comments"].as_array().unwrap().len(),
        0,
        "the refused reply stored nothing"
    );
}

#[test]
fn replies_nest_one_level() {
    let session = session();
    let host = save_issue(
        &session,
        0,
        "nesting-host",
        json!({ "project_id": 1, "title": "Host" }),
    );
    let root = session
        .call(
            "comment.create",
            &json!({
                "mutation": mutation(0, "nesting-root"),
                "project_id": 1,
                "target": { "kind": "ticket", "id": host.ticket_id.to_string() },
                "text": "Root",
                "author": "lin",
            }),
        )
        .unwrap();
    let root_id = root["id"].as_u64().unwrap();
    let reply = session
        .call(
            "comment.create",
            &json!({
                "mutation": mutation(0, "nesting-reply"),
                "project_id": 1,
                "target": { "kind": "ticket", "id": host.ticket_id.to_string() },
                "text": "Reply",
                "author": "lin",
                "parent_comment_id": root_id,
            }),
        )
        .unwrap();
    let reply_id = reply["id"].as_u64().unwrap();

    let error = session
        .call(
            "comment.create",
            &json!({
                "mutation": mutation(0, "nesting-reply-reply"),
                "project_id": 1,
                "target": { "kind": "ticket", "id": host.ticket_id.to_string() },
                "text": "Reply to the reply",
                "author": "lin",
                "parent_comment_id": reply_id,
            }),
        )
        .expect_err("a reply to a reply is refused");

    assert_eq!(error.code, ErrorCode::InvalidRequest);
    assert_eq!(error.field.as_deref(), Some("parent_comment_id"));
    let listed = session
        .call(
            "comment.list",
            &json!({
                "target": { "kind": "ticket", "id": host.ticket_id.to_string() },
                "limit": 10,
            }),
        )
        .unwrap();
    assert_eq!(
        listed["comments"].as_array().unwrap().len(),
        2,
        "the refused second-level reply stored nothing"
    );
}

#[test]
fn comment_author_is_required_and_kept() {
    let session = session();
    let host = save_issue(
        &session,
        0,
        "author-host",
        json!({ "project_id": 1, "title": "Host" }),
    );
    let target = json!({ "kind": "ticket", "id": host.ticket_id.to_string() });

    for (label, author) in [
        ("absent", None),
        ("empty", Some("")),
        ("blank", Some("   ")),
    ] {
        let mut payload = json!({
            "mutation": mutation(0, &format!("author-{label}")),
            "project_id": 1,
            "target": target,
            "text": "Note",
        });
        if let Some(author) = author {
            payload["author"] = json!(author);
        }
        let error = session
            .call("comment.create", &payload)
            .expect_err("a blank author is refused");
        assert_eq!(error.code, ErrorCode::InvalidRequest, "{label} author");
        assert_eq!(error.field.as_deref(), Some("author"), "{label} author");
    }

    session
        .call(
            "comment.create",
            &json!({
                "mutation": mutation(0, "author-set"),
                "project_id": 1,
                "target": target,
                "text": "Note",
                "author": "lin",
            }),
        )
        .expect("a named author is accepted");
    let listed = session
        .call("comment.list", &json!({ "target": target, "limit": 10 }))
        .unwrap();
    assert_eq!(listed["comments"][0]["author"], json!("lin"));
}
