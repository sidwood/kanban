//! Linear issue model rules that must hold across the operator MCP and
//! the legacy Ticket lifecycle: terminal lifecycle stamps, synchronized
//! state history for legacy moves and reassignment, fractional
//! sub-issue ranks, history rows for a new issue's first state, and
//! no-op saves that change nothing.

use std::sync::Arc;

use kanban_app::{
    CommentIssueTargets, CommentStore, Core, IssueStore, IssueStoreCommentTargets, OperatorSession,
};
use kanban_dto::{IssueGetQuery, IssueRecord, IssueSaveResponse};
use kanban_storage::SqliteCommentStore;
use kanban_storage::{
    Database, SqliteDependencyStore, SqliteEvidenceStore, SqliteIssueStore, SqlitePlanStore,
    SqliteProjectStore, SqliteScheduleStore, SqliteSpecStore, SqliteTicketStore,
};
use serde_json::json;

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
        Arc::new(SqliteEvidenceStore::new(
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

struct RulesHarness {
    _dir: tempfile::TempDir,
    core: Arc<Core>,
    session: OperatorSession,
}

fn rules_harness() -> RulesHarness {
    let mut h = harness();
    let attachments = h._dir.path().join("attachments");
    register_issue_stack(&mut h.core, &h.database, &attachments);
    let core = Arc::new(h.core);
    RulesHarness {
        session: OperatorSession::new(core.clone()),
        core,
        _dir: h._dir,
    }
}

fn save_issue(
    session: &OperatorSession,
    version: u64,
    key: &str,
    fields: serde_json::Value,
) -> IssueSaveResponse {
    let mut object = serde_json::Map::new();
    object.insert("mutation".into(), mutation(version, key));
    if let Some(extra) = fields.as_object() {
        object.extend(extra.clone());
    }
    serde_json::from_value(
        session
            .call("issue.save", &serde_json::Value::Object(object))
            .expect("issue.save"),
    )
    .expect("save shape")
}

fn read_issue(session: &OperatorSession, identifier: &str) -> IssueRecord {
    serde_json::from_value(
        session
            .call(
                "issue.get",
                &serde_json::to_value(IssueGetQuery {
                    identifier: identifier.to_owned(),
                })
                .unwrap(),
            )
            .expect("issue.get"),
    )
    .expect("issue record shape")
}

fn issue_version(session: &OperatorSession, identifier: &str) -> u64 {
    read_issue(session, identifier).version
}

#[test]
fn switching_between_terminal_states_keeps_only_the_current_stamp() {
    let h = rules_harness();
    let session = &h.session;

    let saved = save_issue(
        session,
        0,
        "terminal-switch-create",
        json!({ "project_id": 1, "title": "Terminal switch", "state": "Done" }),
    );
    let done = read_issue(session, &saved.identifier);
    assert!(
        done.completed_at.is_some(),
        "entering Done stamps completion"
    );
    assert!(done.canceled_at.is_none());

    save_issue(
        session,
        done.version,
        "terminal-switch-cancel",
        json!({ "identifier": saved.identifier, "state": "Cancelled" }),
    );
    let moved = read_issue(session, &saved.identifier);
    assert!(
        moved.completed_at.is_none(),
        "a direct Done to Cancelled move drops the completed stamp"
    );
    assert!(
        moved.canceled_at.is_some(),
        "entering Cancelled stamps cancellation"
    );

    save_issue(
        session,
        moved.version,
        "terminal-switch-done",
        json!({ "identifier": saved.identifier, "state": "Done" }),
    );
    let redone = read_issue(session, &saved.identifier);
    assert!(
        redone.completed_at.is_some(),
        "entering Done again restamps completion"
    );
    assert!(
        redone.canceled_at.is_none(),
        "a direct Cancelled to Done move drops the canceled stamp"
    );
}

fn register_legacy_stack(core: &mut Core, database: &Database, attachments: &std::path::Path) {
    let projects = Arc::new(SqliteProjectStore::new(database));
    let tickets = Arc::new(SqliteTicketStore::new(database));
    let specs = Arc::new(SqliteSpecStore::new(database));
    let plans = Arc::new(SqlitePlanStore::new(database));
    let issues: Arc<dyn IssueStore> = Arc::new(SqliteIssueStore::new(database));
    let comments: Arc<dyn CommentStore> = Arc::new(SqliteCommentStore::new(database));
    let dependencies = Arc::new(SqliteDependencyStore::new(database));
    let schedules = Arc::new(SqliteScheduleStore::new(database));
    core.register_plans(plans.clone(), projects.clone(), specs.clone())
        .expect("plans register");
    core.register_specs(specs.clone(), projects.clone(), plans)
        .expect("specs register");
    std::fs::create_dir_all(attachments).expect("attachments dir");
    core.register_tickets(
        tickets.clone(),
        projects.clone(),
        specs.clone(),
        Arc::new(SqliteEvidenceStore::new(
            database,
            attachments.to_path_buf(),
        )),
    )
    .expect("tickets register");
    core.register_lifecycle(
        tickets.clone(),
        dependencies,
        projects.clone(),
        schedules,
        None,
    )
    .expect("lifecycle registers");
    core.register_reassignment(tickets.clone(), projects.clone(), specs.clone())
        .expect("reassignment registers");
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

fn legacy_harness() -> RulesHarness {
    let mut h = harness();
    let attachments = h._dir.path().join("attachments");
    register_legacy_stack(&mut h.core, &h.database, &attachments);
    let core = Arc::new(h.core);
    RulesHarness {
        session: OperatorSession::new(core.clone()),
        core,
        _dir: h._dir,
    }
}

fn create_legacy_task(core: &Core, key: &str, title: &str) -> serde_json::Value {
    core.command(
        "ticket.create",
        &json!({
            "mutation": mutation(0, key),
            "project_id": 1,
            "kind": "task",
            "priority": "normal",
            "title": title,
            "subtype": "operational",
            "mode": "human",
            "completion": ["Tracked"],
        }),
    )
    .expect("the legacy Task lands")
}

fn history_names(record: &IssueRecord) -> Vec<&str> {
    record
        .state_history
        .iter()
        .map(|row| row.state.as_str())
        .collect()
}

#[test]
fn legacy_parking_moves_the_issue_state_and_history() {
    let h = legacy_harness();
    let session = &h.session;
    let created = create_legacy_task(&h.core, "legacy-park-create", "Legacy parked task");
    let ticket_id = created["id"].as_u64().expect("a ticket id");
    let identifier = format!("CORE-{}", created["number"].as_u64().expect("a number"));

    let claimed = save_issue(
        session,
        created["version"].as_u64().expect("a version"),
        "legacy-park-ready",
        json!({ "identifier": identifier, "state": "Ready" }),
    );
    let _ = claimed;

    h.core
        .command(
            "ticket.park",
            &json!({
                "mutation": mutation(issue_version(session, &identifier), "legacy-park-park"),
                "ticket_id": ticket_id,
            }),
        )
        .expect("the legacy park lands");

    let moved = read_issue(session, &identifier);
    assert_eq!(
        moved.state, "Parked",
        "the issue model follows the legacy move"
    );
    assert_eq!(moved.state_type, "backlog");
    let names = history_names(&moved);
    assert!(
        names.ends_with(&["Ready", "Parked"]),
        "the move closes the Ready row and opens a Parked row, saw {names:?}"
    );
    assert!(moved.state_history.last().unwrap().ended_at.is_none());
    assert!(
        moved.state_history[..moved.state_history.len() - 1]
            .iter()
            .all(|row| row.ended_at.is_some()),
        "every earlier state is closed"
    );

    let rows_before_edit = moved.state_history.len();
    let version = moved.version;
    h.core
        .command(
            "ticket.edit",
            &json!({
                "mutation": mutation(version, "legacy-park-edit"),
                "ticket_id": ticket_id,
                "title": "Retitled without moving",
            }),
        )
        .expect("the legacy edit lands");
    let edited = read_issue(session, &identifier);
    assert_eq!(edited.state, "Parked");
    assert_eq!(
        edited.state_history.len(),
        rows_before_edit,
        "a same-state legacy save writes no history"
    );
}

#[test]
fn legacy_cancelling_stamps_the_issue_cancellation() {
    let h = legacy_harness();
    let session = &h.session;
    let created = create_legacy_task(&h.core, "legacy-cancel-create", "Legacy cancelled task");
    let ticket_id = created["id"].as_u64().expect("a ticket id");
    let identifier = format!("CORE-{}", created["number"].as_u64().expect("a number"));

    save_issue(
        session,
        created["version"].as_u64().expect("a version"),
        "legacy-cancel-ready",
        json!({ "identifier": identifier, "state": "Ready" }),
    );
    h.core
        .command(
            "ticket.cancel",
            &json!({
                "mutation": mutation(issue_version(session, &identifier), "legacy-cancel-cancel"),
                "ticket_id": ticket_id,
            }),
        )
        .expect("the legacy cancel lands");

    let moved = read_issue(session, &identifier);
    assert_eq!(
        moved.state, "Cancelled",
        "the issue model follows the cancel"
    );
    assert_eq!(moved.state_type, "canceled");
    assert!(
        moved.canceled_at.is_some(),
        "the legacy cancel stamps cancellation for the issue model"
    );
    assert!(moved.completed_at.is_none());
    let names = history_names(&moved);
    assert!(
        names.ends_with(&["Ready", "Cancelled"]),
        "the cancel closes the Ready row and opens a Cancelled row, saw {names:?}"
    );
}

#[test]
fn fractional_child_ranks_survive_round_trips_and_swaps() {
    let h = rules_harness();
    let session = &h.session;

    let parent = save_issue(
        session,
        0,
        "ranks-parent",
        json!({ "project_id": 1, "title": "Ranked parent" }),
    );
    let first = save_issue(
        session,
        0,
        "ranks-first",
        json!({
            "project_id": 1,
            "title": "First child",
            "parent_id": parent.ticket_id,
            "sort_order": 0.25,
        }),
    );
    let second = save_issue(
        session,
        0,
        "ranks-second",
        json!({
            "project_id": 1,
            "title": "Second child",
            "parent_id": parent.ticket_id,
            "sort_order": 0.75,
        }),
    );

    let read: serde_json::Value = session
        .call("issue.get", &json!({ "identifier": parent.identifier }))
        .expect("the parent reads");
    let children = read["children"].as_array().expect("children listed");
    assert_eq!(children.len(), 2);
    assert_eq!(children[0]["identifier"], json!(first.identifier));
    assert_eq!(
        children[0]["sort_order"],
        json!(0.25),
        "a fractional rank reads back exactly from get_issue"
    );
    assert_eq!(children[1]["identifier"], json!(second.identifier));
    assert_eq!(children[1]["sort_order"], json!(0.75));

    save_issue(
        session,
        issue_version(session, &first.identifier),
        "ranks-first-swap",
        json!({ "identifier": first.identifier, "sort_order": 0.75 }),
    );
    save_issue(
        session,
        issue_version(session, &second.identifier),
        "ranks-second-swap",
        json!({ "identifier": second.identifier, "sort_order": 0.25 }),
    );

    let swapped: serde_json::Value = session
        .call("issue.get", &json!({ "identifier": parent.identifier }))
        .expect("the parent reads again");
    let children = swapped["children"].as_array().expect("children listed");
    assert_eq!(
        children[0]["identifier"],
        json!(second.identifier),
        "swapping fractional ranks swaps the children order"
    );
    assert_eq!(children[0]["sort_order"], json!(0.25));
    assert_eq!(children[1]["sort_order"], json!(0.75));

    let listed: serde_json::Value = session
        .call("issue.list", &json!({ "project_id": 1 }))
        .expect("the project lists");
    let listed_parent = listed["issues"]
        .as_array()
        .expect("a page")
        .iter()
        .find(|issue| issue["identifier"] == json!(parent.identifier))
        .expect("the parent is listed");
    let listed_children = listed_parent["children"]
        .as_array()
        .expect("children listed");
    assert_eq!(
        listed_children[0]["identifier"],
        json!(second.identifier),
        "list_issues carries the swapped fractional order"
    );
    assert_eq!(listed_children[0]["sort_order"], json!(0.25));
}

#[test]
fn creating_with_a_state_records_only_that_state() {
    let h = rules_harness();
    let session = &h.session;

    let saved = save_issue(
        session,
        0,
        "create-state-active",
        json!({ "project_id": 1, "title": "Born active", "state": "Active" }),
    );
    let read = read_issue(session, &saved.identifier);
    assert_eq!(
        history_names(&read),
        &["Active".to_string()],
        "creation with a state records only that state"
    );
    assert!(read.state_history[0].ended_at.is_none());
    assert!(
        read.started_at.is_some(),
        "entering a started state at birth stamps the start"
    );

    let done = save_issue(
        session,
        0,
        "create-state-done",
        json!({ "project_id": 1, "title": "Born done", "state": "Done" }),
    );
    let read_done = read_issue(session, &done.identifier);
    assert_eq!(history_names(&read_done), &["Done".to_string()]);
    assert!(
        read_done.completed_at.is_some(),
        "entering a completed state at birth stamps completion"
    );
}

#[test]
fn creating_without_a_state_records_the_initial_state() {
    let h = rules_harness();
    let session = &h.session;

    let saved = save_issue(
        session,
        0,
        "create-state-default",
        json!({ "project_id": 1, "title": "Born default" }),
    );
    let read = read_issue(session, &saved.identifier);
    assert_eq!(read.state, "Draft");
    assert_eq!(
        history_names(&read),
        &["Draft".to_string()],
        "creation without a state records one row for the initial state"
    );
    assert!(read.state_history[0].ended_at.is_none());
}

#[test]
fn saving_unchanged_fields_keeps_version_and_history() {
    let h = rules_harness();
    let session = &h.session;

    session
        .call(
            "issue.create_milestone",
            &json!({
                "mutation": mutation(0, "noop-milestone"),
                "project_id": 1,
                "name": "Unchanged milestone",
            }),
        )
        .expect("the milestone is created");

    let saved = save_issue(
        session,
        0,
        "noop-create",
        json!({
            "project_id": 1,
            "title": "Unchanged title",
            "description": "Unchanged description",
            "state": "Active",
            "priority": 2,
            "due_date": "2026-12-01",
            "milestone_id": 1,
            "assignee": "sid",
        }),
    );
    let before = read_issue(session, &saved.identifier);
    assert_eq!(before.state, "Active");
    assert_eq!(before.priority, 2);
    assert_eq!(before.due_date.as_deref(), Some("2026-12-01"));
    assert_eq!(before.milestone_id, Some(1));
    assert_eq!(before.assignee.as_deref(), Some("sid"));

    for key in ["noop-resave", "noop-resave-again"] {
        save_issue(
            session,
            before.version,
            key,
            json!({
                "identifier": saved.identifier,
                "title": "Unchanged title",
                "description": "Unchanged description",
                "state": "Active",
                "priority": 2,
                "due_date": "2026-12-01",
                "milestone_id": 1,
                "assignee": "sid",
            }),
        );
        let after = read_issue(session, &saved.identifier);
        assert_eq!(
            after.version, before.version,
            "a save that changes nothing keeps the version ({key})"
        );
        assert_eq!(
            after.updated_at, before.updated_at,
            "a save that changes nothing keeps updated_at ({key})"
        );
        assert_eq!(
            after.state_history.len(),
            before.state_history.len(),
            "a save that changes nothing writes no history ({key})"
        );
    }
}

/// A legacy `ticket.reassign` supersedes the original, so the issue
/// projection must follow the original into the superseded workflow
/// state with its history and stamps, exactly as a park or cancel does.
#[test]
fn legacy_reassignment_moves_the_original_issue_state() {
    let h = legacy_harness();
    let session = &h.session;
    let created = create_legacy_task(&h.core, "legacy-reassign-create", "Replaced task");
    let ticket_id = created["id"].as_u64().expect("a ticket id");
    let identifier = format!("CORE-{}", created["number"].as_u64().expect("a number"));

    save_issue(
        session,
        created["version"].as_u64().expect("a version"),
        "legacy-reassign-ready",
        json!({ "identifier": identifier, "state": "Ready" }),
    );

    let replacement = h
        .core
        .command(
            "ticket.reassign",
            &json!({
                "mutation": mutation(issue_version(session, &identifier), "legacy-reassign"),
                "ticket_id": ticket_id,
                "kind": "task",
                "priority": "normal",
                "title": "Replacement task",
                "subtype": "operational",
                "mode": "human",
                "completion": ["Checked"],
            }),
        )
        .expect("the reassignment lands");
    assert!(
        replacement["id"].as_u64().is_some_and(|id| id != ticket_id),
        "the reassignment returns the replacement"
    );

    let moved = read_issue(session, &identifier);
    assert_eq!(
        moved.state, "Superseded",
        "the issue model follows the superseded original"
    );
    assert_eq!(moved.state_type, "canceled");
    assert!(
        moved.canceled_at.is_some(),
        "superseding stamps cancellation for the issue model"
    );
    assert!(moved.completed_at.is_none());
    let names = history_names(&moved);
    assert!(
        names.ends_with(&["Ready", "Superseded"]),
        "the reassignment closes the Ready row and opens a Superseded row, saw {names:?}"
    );
    assert!(moved.state_history.last().unwrap().ended_at.is_none());
}

