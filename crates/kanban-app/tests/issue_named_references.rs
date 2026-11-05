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
fn save_comment_on_core_2_returns_the_key_url_and_author() {
    let mut h = harness();
    let attachments = h._dir.path().join("attachments");
    register_issue_stack(&mut h.core, &h.database, &attachments);
    let session = OperatorSession::with_operator_name(Arc::new(h.core), "Ada");
    let first: IssueSaveResponse = serde_json::from_value(call(
        &session,
        "issue.save",
        "comment-first",
        json!({ "project_id": 1, "title": "First" }),
    ))
    .expect("save shape");
    let second: IssueSaveResponse = serde_json::from_value(call(
        &session,
        "issue.save",
        "comment-second",
        json!({ "project_id": 1, "title": "Second" }),
    ))
    .expect("save shape");
    assert_eq!(first.identifier, "CORE-1");
    assert_eq!(second.identifier, "CORE-2");

    let posted = session
        .call(
            "save_comment",
            &json!({ "issue": "CORE-2", "body": "Ship report" }),
        )
        .expect("save_comment");
    assert!(
        posted["id"].as_u64().is_some(),
        "the comment id is returned: {posted}"
    );
    assert_eq!(posted["identifier"], json!("CORE-2"));
    assert_eq!(posted["url"], json!("kanban://issue/CORE-2"));

    let listed = list_comments(&session, "CORE-2");
    assert_eq!(listed["comments"][0]["author"], json!("Ada"));
    assert!(listed["comments"][0].get("parent_comment_id").is_none());

    let parent_id = posted["id"].as_u64().expect("comment id");
    let reply = session
        .call(
            "save_comment",
            &json!({
                "issue": "CORE-2",
                "body": "Noted",
                "author": "lin",
                "parent_comment_id": parent_id
            }),
        )
        .expect("reply");
    assert_eq!(reply["identifier"], json!("CORE-2"));
    assert_eq!(reply["url"], json!("kanban://issue/CORE-2"));
    let listed = list_comments(&session, "CORE-2");
    assert_eq!(listed["comments"][1]["author"], json!("lin"));
    assert_eq!(listed["comments"][1]["parent_comment_id"], json!(parent_id));
}

fn list_comments(session: &OperatorSession, issue: &str) -> Value {
    session
        .call(
            "comment.list",
            &json!({
                "target": {"kind": "ticket", "id": issue},
                "limit": 10
            }),
        )
        .expect("comment.list")
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

#[test]
fn an_issue_tag_adds_one_related_link() {
    let session = session();
    let mentioned: IssueSaveResponse = serde_json::from_value(call(
        &session,
        "issue.save",
        "tag-mentioned",
        json!({ "project_id": 1, "title": "Mentioned" }),
    ))
    .expect("save shape");
    let host: IssueSaveResponse = serde_json::from_value(call(
        &session,
        "issue.save",
        "tag-host",
        json!({
            "project_id": 1,
            "title": "Host",
            "description": format!(
                "See <issue id=\"{}\">{}</issue> later.",
                mentioned.identifier, mentioned.identifier
            )
        }),
    ))
    .expect("save shape");
    let read = session
        .call("issue.get", &json!({ "identifier": host.identifier }))
        .expect("issue.get");
    let related = read["relations"]
        .as_array()
        .expect("relations")
        .iter()
        .filter(|row| row["kind"] == "related" && row["identifier"] == mentioned.identifier)
        .count();
    assert_eq!(related, 1, "the issue tag adds one related link: {read}");

    let version = read["version"].as_u64().expect("version");
    session
        .call(
            "issue.save",
            &json!({
                "mutation": mutation(version, "tag-again"),
                "identifier": host.identifier,
                "description": format!(
                    "See <issue id=\"{}\">{}</issue> again.",
                    mentioned.identifier, mentioned.identifier
                )
            }),
        )
        .expect("the repeat lands");
    let again = session
        .call("issue.get", &json!({ "identifier": host.identifier }))
        .expect("issue.get");
    let related_again = again["relations"]
        .as_array()
        .expect("relations")
        .iter()
        .filter(|row| row["kind"] == "related" && row["identifier"] == mentioned.identifier)
        .count();
    assert_eq!(
        related_again, 1,
        "a repeated mention does not add a second link"
    );
}

#[test]
fn an_unknown_state_names_the_kanban_states() {
    let session = session();
    let error = session
        .call(
            "issue.save",
            &json!({
                "mutation": mutation(0, "unknown-state"),
                "project_id": 1,
                "title": "Unknown state",
                "state": "Todo"
            }),
        )
        .expect_err("an unknown state is refused");
    for name in [
        "Draft",
        "Parked",
        "Blocked",
        "Scheduled",
        "Ready",
        "Active",
        "In Review",
        "Approved",
        "Landing",
        "Done",
        "Cancelled",
        "Superseded",
        "Duplicate",
    ] {
        assert!(
            error.message.contains(name),
            "the refusal names {name}: {}",
            error.message
        );
    }
    let tools = std::fs::read_to_string(format!(
        "{}/../../packages/contracts/src/mcp-tools.json",
        env!("CARGO_MANIFEST_DIR")
    ))
    .expect("published tools");
    let tools: serde_json::Value = serde_json::from_str(&tools).expect("tools json");
    let save = tools
        .as_array()
        .expect("tools")
        .iter()
        .find(|tool| tool["name"] == "save_issue")
        .expect("save_issue");
    let described = save["inputSchema"]["properties"]["state"]["description"]
        .as_str()
        .unwrap_or("");
    assert!(
        described.contains("Draft") && described.contains("Duplicate"),
        "tools/list describes state with the Kanban names: {described}"
    );
}

fn issue_version(session: &OperatorSession, identifier: &str) -> u64 {
    session
        .call("issue.get", &json!({ "identifier": identifier }))
        .expect("issue.get")["version"]
        .as_u64()
        .expect("version")
}

fn apply_edits(session: &OperatorSession, identifier: &str, key: &str, edits: Value) -> Value {
    let version = issue_version(session, identifier);
    session
        .call(
            "issue.save",
            &json!({
                "mutation": mutation(version, key),
                "identifier": identifier,
                "description_edits": edits
            }),
        )
        .expect("the edits apply")
}

#[test]
fn each_description_edit_applies_at_a_unique_anchor() {
    let session = session();
    let saved: IssueSaveResponse = serde_json::from_value(call(
        &session,
        "issue.save",
        "each-edit-create",
        json!({
            "project_id": 1,
            "title": "Edited",
            "description": "HEAD middle TAIL"
        }),
    ))
    .expect("save shape");
    let identifier = saved.identifier;
    apply_edits(
        &session,
        &identifier,
        "edit-replace",
        json!([{"type": "replace", "find": "HEAD", "replace": "START"}]),
    );
    assert_eq!(
        issue_description(&session, &identifier),
        json!("START middle TAIL")
    );
    apply_edits(
        &session,
        &identifier,
        "edit-insert-before",
        json!([{"type": "insert_before", "anchor": "middle", "text": "BEFORE "}]),
    );
    assert_eq!(
        issue_description(&session, &identifier),
        json!("START BEFORE middle TAIL")
    );
    apply_edits(
        &session,
        &identifier,
        "edit-insert-after",
        json!([{"type": "insert_after", "anchor": "middle", "text": " AFTER"}]),
    );
    assert_eq!(
        issue_description(&session, &identifier),
        json!("START BEFORE middle AFTER TAIL")
    );
    apply_edits(
        &session,
        &identifier,
        "edit-prepend",
        json!([{"type": "prepend", "text": "PRE "}]),
    );
    assert_eq!(
        issue_description(&session, &identifier),
        json!("PRE START BEFORE middle AFTER TAIL")
    );
    apply_edits(
        &session,
        &identifier,
        "edit-append",
        json!([{"type": "append", "text": " APP"}]),
    );
    assert_eq!(
        issue_description(&session, &identifier),
        json!("PRE START BEFORE middle AFTER TAIL APP")
    );
    apply_edits(
        &session,
        &identifier,
        "edit-range",
        json!([{
            "type": "replace_range",
            "start": "BEFORE",
            "end": "AFTER",
            "text": "SPAN"
        }]),
    );
    assert_eq!(
        issue_description(&session, &identifier),
        json!("PRE START SPAN TAIL APP")
    );
}

#[test]
fn a_later_failing_description_edit_leaves_the_issue_unchanged() {
    let session = session();
    let mentioned: IssueSaveResponse = serde_json::from_value(call(
        &session,
        "issue.save",
        "edit-mentioned",
        json!({ "project_id": 1, "title": "Mentioned" }),
    ))
    .expect("save shape");
    let extra: IssueSaveResponse = serde_json::from_value(call(
        &session,
        "issue.save",
        "edit-extra",
        json!({ "project_id": 1, "title": "Extra" }),
    ))
    .expect("save shape");
    let saved: IssueSaveResponse = serde_json::from_value(call(
        &session,
        "issue.save",
        "edit-host",
        json!({
            "project_id": 1,
            "title": "Host",
            "description": "alpha beta alpha"
        }),
    ))
    .expect("save shape");
    let version = issue_version(&session, &saved.identifier);
    session
        .call(
            "issue.save",
            &json!({
                "mutation": mutation(version, "edit-relate"),
                "identifier": saved.identifier,
                "add_related": [mentioned.identifier]
            }),
        )
        .expect("the existing relation lands");
    let before = session
        .call("issue.get", &json!({ "identifier": saved.identifier }))
        .expect("issue.get");
    let before_version = before["version"].clone();
    let before_relations = before["relations"].clone();
    let refused = session.call(
        "issue.save",
        &json!({
            "mutation": mutation(before_version.as_u64().expect("version"), "edit-partial"),
            "identifier": saved.identifier,
            "description_edits": [
                {
                    "type": "append",
                    "text": format!(" <issue id=\"{}\">{}</issue>", extra.identifier, extra.identifier)
                },
                {"type": "replace", "find": "alpha", "replace": "ALPHA"}
            ]
        }),
    );
    assert!(
        refused.is_err(),
        "a later edit whose anchor matches twice is refused"
    );
    let after = session
        .call("issue.get", &json!({ "identifier": saved.identifier }))
        .expect("issue.get");
    assert_eq!(after["description"], json!("alpha beta alpha"));
    assert_eq!(after["version"], before_version);
    assert_eq!(after["relations"], before_relations);
    assert!(
        !after["relations"]
            .as_array()
            .expect("relations")
            .iter()
            .any(|row| row["identifier"] == extra.identifier),
        "the earlier edit must not add a relation: {after}"
    );
}

#[test]
fn description_patch_still_replaces_the_first_match() {
    let session = session();
    let saved: IssueSaveResponse = serde_json::from_value(call(
        &session,
        "issue.save",
        "patch-create",
        json!({
            "project_id": 1,
            "title": "Patched",
            "description": "alpha beta alpha"
        }),
    ))
    .expect("save shape");
    let version = issue_version(&session, &saved.identifier);
    session
        .call(
            "issue.save",
            &json!({
                "mutation": mutation(version, "patch-apply"),
                "identifier": saved.identifier,
                "description_patch": json!({"find": "alpha", "replace": "ALPHA"}).to_string()
            }),
        )
        .expect("description_patch still applies");
    assert_eq!(
        issue_description(&session, &saved.identifier),
        json!("ALPHA beta alpha")
    );
}

fn issue_description(session: &OperatorSession, identifier: &str) -> Value {
    session
        .call("issue.get", &json!({ "identifier": identifier }))
        .expect("issue.get")["description"]
        .clone()
}

#[test]
fn linear_aliases_create_labels_milestones_and_issue_links() {
    let session = session();
    let label = session
        .call(
            "create_issue_label",
            &json!({
                "project_id": 1,
                "name": "Web",
                "color": "#336699",
                "group": "Project",
                "group_single_select": true
            }),
        )
        .expect("create_issue_label");
    assert_eq!(label["name"], json!("Web"));
    let labels = session
        .call("issue.list_labels", &json!({ "project_id": 1 }))
        .expect("issue.list_labels");
    assert!(
        labels["labels"]
            .as_array()
            .expect("labels")
            .iter()
            .any(|row| row["name"] == "Web"),
        "the alias creates a listed label: {labels}"
    );

    let milestone = session
        .call(
            "save_milestone",
            &json!({ "project_id": 1, "name": "M1 Foundations" }),
        )
        .expect("save_milestone");
    assert_eq!(milestone["name"], json!("M1 Foundations"));
    let milestones = session
        .call("issue.list_milestones", &json!({ "project_id": 1 }))
        .expect("issue.list_milestones");
    assert!(
        milestones["milestones"]
            .as_array()
            .expect("milestones")
            .iter()
            .any(|row| row["name"] == "M1 Foundations"),
        "the alias creates a listed milestone: {milestones}"
    );

    let parent: IssueSaveResponse = serde_json::from_value(call(
        &session,
        "issue.save",
        "alias-parent",
        json!({ "project_id": 1, "title": "Parent" }),
    ))
    .expect("save shape");
    let blocker: IssueSaveResponse = serde_json::from_value(call(
        &session,
        "issue.save",
        "alias-blocker",
        json!({ "project_id": 1, "title": "Blocker" }),
    ))
    .expect("save shape");
    let child = session
        .call(
            "issue.save",
            &json!({
                "title": "Child",
                "parentId": parent.identifier,
                "dueDate": "2026-12-01",
                "blockedBy": [blocker.identifier]
            }),
        )
        .expect("field aliases");
    let child_key = child["identifier"].as_str().expect("child key").to_owned();
    let read = session
        .call("issue.get", &json!({ "id": child_key }))
        .expect("issue.get by id");
    assert_eq!(read["parent"], json!(parent.identifier));
    assert_eq!(read["due_date"], json!("2026-12-01"));
    assert!(
        read["relations"]
            .as_array()
            .expect("relations")
            .iter()
            .any(|row| row["kind"] == "blocked-by" && row["identifier"] == blocker.identifier),
        "blockedBy adds a blocked-by link: {read}"
    );

    session
        .call(
            "issue.save",
            &json!({ "id": child_key, "title": "Child renamed" }),
        )
        .expect("id updates the issue");
    let renamed = session
        .call("issue.get", &json!({ "id": child_key }))
        .expect("issue.get by id");
    assert_eq!(renamed["title"], json!("Child renamed"));

    let children = session
        .call("issue.list", &json!({ "parentId": parent.identifier }))
        .expect("issue.list by parentId");
    assert!(
        children["issues"]
            .as_array()
            .expect("issues")
            .iter()
            .any(|row| row["identifier"] == child_key),
        "parentId lists the child: {children}"
    );
}

#[test]
fn operator_tool_schemas_describe_every_field_and_the_kanban_states() {
    let tools = std::fs::read_to_string(format!(
        "{}/../../packages/contracts/src/mcp-tools.json",
        env!("CARGO_MANIFEST_DIR")
    ))
    .expect("published tools");
    let tools: Value = serde_json::from_str(&tools).expect("tools json");
    let tools = tools.as_array().expect("tools");
    let operator = [
        "get_issue",
        "list_issues",
        "list_issue_labels",
        "list_milestones",
        "list_projects",
        "list_issue_statuses",
        "save_issue",
        "create_milestone",
        "create_label",
        "retire_label",
        "update_project_settings",
        "comment_create",
        "comment_edit",
        "list_comments",
        "comment_revisions",
        "resolve_comment",
    ];
    let mut missing = Vec::new();
    for name in operator {
        let tool = tools
            .iter()
            .find(|tool| tool["name"] == name)
            .unwrap_or_else(|| panic!("{name} is published"));
        for side in ["inputSchema", "outputSchema"] {
            let schema = &tool[side];
            missing_descriptions(schema, schema, &format!("{name}/{side}"), &mut missing);
        }
    }
    assert!(
        missing.is_empty(),
        "operator fields lack descriptions: {missing:?}"
    );
    let described = tools
        .iter()
        .find(|tool| tool["name"] == "save_issue")
        .expect("save_issue")["inputSchema"]["properties"]["state"]["description"]
        .as_str()
        .unwrap_or("");
    for name in [
        "Draft",
        "Parked",
        "Blocked",
        "Scheduled",
        "Ready",
        "Active",
        "In Review",
        "Approved",
        "Landing",
        "Done",
        "Cancelled",
        "Superseded",
        "Duplicate",
    ] {
        assert!(described.contains(name), "state lists {name}: {described}");
    }
}

fn missing_descriptions(schema: &Value, root: &Value, path: &str, missing: &mut Vec<String>) {
    let mut seen = std::collections::HashSet::new();
    walk_schema(schema, root, path, &mut seen, missing);
}

fn walk_schema(
    schema: &Value,
    root: &Value,
    path: &str,
    seen: &mut std::collections::HashSet<String>,
    missing: &mut Vec<String>,
) {
    let Some(schema) = schema.as_object() else {
        return;
    };
    if let Some(reference) = schema.get("$ref").and_then(|value| value.as_str()) {
        let key = format!("{path}->{reference}");
        if seen.insert(key)
            && let Some(target) = resolve_ref(root, reference)
        {
            walk_schema(target, root, &format!("{path}{reference}"), seen, missing);
        }
    }
    if let Some(properties) = schema.get("properties").and_then(|value| value.as_object()) {
        for (name, field) in properties {
            let field_path = format!("{path}.{name}");
            let described = field
                .get("description")
                .and_then(|value| value.as_str())
                .is_some_and(|text| !text.trim().is_empty());
            if !described {
                missing.push(field_path.clone());
            }
            walk_schema(field, root, &field_path, seen, missing);
        }
    }
    if let Some(items) = schema.get("items") {
        if let Some(items) = items.as_array() {
            for (index, item) in items.iter().enumerate() {
                walk_schema(item, root, &format!("{path}[]{index}"), seen, missing);
            }
        } else {
            walk_schema(items, root, &format!("{path}[]"), seen, missing);
        }
    }
    for key in ["anyOf", "oneOf", "allOf"] {
        if let Some(options) = schema.get(key).and_then(|value| value.as_array()) {
            for (index, option) in options.iter().enumerate() {
                walk_schema(option, root, &format!("{path}/{key}{index}"), seen, missing);
            }
        }
    }
    if let Some(definitions) = schema
        .get("definitions")
        .and_then(|value| value.as_object())
    {
        for (name, definition) in definitions {
            walk_schema(definition, root, &format!("{path}#/{name}"), seen, missing);
        }
    }
    if let Some(additional) = schema.get("additionalProperties") {
        walk_schema(
            additional,
            root,
            &format!("{path}.additionalProperties"),
            seen,
            missing,
        );
    }
}

fn resolve_ref<'a>(root: &'a Value, reference: &str) -> Option<&'a Value> {
    let pointer = reference.strip_prefix("#/")?;
    let mut node = root;
    for part in pointer.split('/') {
        node = node.get(part)?;
    }
    Some(node)
}

#[test]
fn overlapping_description_anchors_leave_the_issue_unchanged() {
    let session = session();
    let mentioned = session
        .call("issue.save", &json!({"team":"CORE", "title":"Mentioned"}))
        .unwrap();
    for (description, anchor) in [("banana", "ana"), ("🍌🍎🍌🍎🍌", "🍌🍎🍌")] {
        let saved = session
            .call(
                "issue.save",
                &json!({
                    "team":"CORE", "title":"Overlapping text", "description":description
                }),
            )
            .unwrap();
        let before = session
            .call("issue.get", &json!({"id":saved["identifier"]}))
            .unwrap();
        for edit in [
            json!({"type":"replace", "find":anchor, "replace":"X"}),
            json!({"type":"insert_before", "anchor":anchor, "text":"X"}),
            json!({"type":"insert_after", "anchor":anchor, "text":"X"}),
            json!({"type":"replace_range", "start":anchor, "end":"!", "text":"X"}),
            json!({"type":"replace_range", "start":"<issue", "end":anchor, "text":"X"}),
        ] {
            let result = session.call("issue.save", &json!({
                "id":saved["identifier"],
                "description_edits":[
                    {"type":"append", "text":format!("! <issue id=\"{}\">Related</issue>", mentioned["identifier"].as_str().unwrap())},
                    edit
                ]
            }));
            let error = result.expect_err("overlapping anchors must be refused");
            assert!(error.message.contains("matched 2 times"), "{error:?}");
            assert_eq!(
                session
                    .call("issue.get", &json!({"id":saved["identifier"]}))
                    .unwrap(),
                before
            );
            assert!(
                session
                    .call("issue.get", &json!({"id":mentioned["identifier"]}))
                    .unwrap()["relations"]
                    .as_array()
                    .unwrap()
                    .is_empty()
            );
        }
        session
            .call(
                "issue.save",
                &json!({
                    "id":saved["identifier"],
                    "description_patch":json!({"find":anchor,"replace":"X"}).to_string()
                }),
            )
            .unwrap();
        let expected = if description == "banana" {
            "bXna"
        } else {
            "X🍎🍌"
        };
        assert_eq!(
            session
                .call("issue.get", &json!({"id":saved["identifier"]}))
                .unwrap()["description"],
            expected
        );
    }
}

#[test]
fn project_filters_keep_the_label_group_across_teams() {
    use kanban_app::ProjectStore;
    let mut h = harness();
    let projects = SqliteProjectStore::new(&h.database);
    let mut teams = vec![1];
    for code in ["CORE2", "CORE3"] {
        let registration = kanban_domain::ProjectRegistration::new(
            code,
            "Another team",
            &format!("/repositories/{code}"),
            &format!("/workspaces/{code}"),
            "main",
            code,
            None,
            None,
        )
        .unwrap();
        let project = projects
            .create(&registration, &|id| {
                kanban_app::TimelineEnvelope::project(
                    id.value(),
                    kanban_dto::TimelineEventKind::Transition,
                    None,
                    json!({"action":"registered"}),
                )
            })
            .unwrap();
        teams.push(project.id().value());
    }
    register_issue_stack(&mut h.core, &h.database, &h._dir.path().join("attachments"));
    let session = OperatorSession::new(Arc::new(h.core));
    let mut saved = Vec::new();
    for (project_id, group) in teams.iter().zip([Some("Area"), Some("Project"), None]) {
        let mut label = json!({"project_id":project_id,"name":"Web","color":"#336699"});
        if let Some(group) = group {
            label["group"] = json!(group);
            label["group_single_select"] = json!(true);
        }
        session.call("create_issue_label", &label).unwrap();
        saved.push(
            session
                .call(
                    "issue.save",
                    &json!({
                        "project_id":project_id,"title":"Web work","add_labels":["Web"]
                    }),
                )
                .unwrap(),
        );
    }
    let list = |filter: Value| {
        session.call("issue.list", &filter).unwrap()["issues"]
            .as_array()
            .unwrap()
            .iter()
            .map(|issue| issue["identifier"].clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(
        list(json!({"project":"Web"})),
        vec![saved[1]["identifier"].clone()]
    );
    assert!(list(json!({"team":"CORE","project":"Web"})).is_empty());
    assert_eq!(
        list(json!({"team":"CORE2","project":"Web"})),
        vec![saved[1]["identifier"].clone()]
    );
    assert_eq!(list(json!({"label":"Web"})).len(), 3);
    assert_eq!(
        list(json!({"project":"Web","label":"Web"})),
        vec![saved[1]["identifier"].clone()]
    );
    assert!(list(json!({"project":"Web","label":"Missing"})).is_empty());
}
