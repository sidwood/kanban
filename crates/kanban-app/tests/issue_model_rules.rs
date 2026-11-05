//! Linear issue model rules that must hold across the operator MCP and
//! the legacy Ticket lifecycle: terminal lifecycle stamps, synchronized
//! state history for legacy moves and reassignment, fractional
//! sub-issue ranks, history rows for a new issue's first state, and
//! no-op saves that change nothing.

use std::sync::Arc;

use kanban_app::{
    CommentIssueTargets, CommentStore, Core, IssueStore, IssueStoreCommentTargets, OperatorSession,
};
use kanban_dto::{ApiError, ErrorCode, IssueGetQuery, IssueRecord, IssueSaveResponse};
use kanban_storage::SqliteCommentStore;
use kanban_storage::{
    Database, SqliteDependencyStore, SqliteEvidenceStore, SqliteIssueStore, SqlitePlanStore,
    SqliteProjectStore, SqliteScheduleStore, SqliteSpecStore, SqliteTicketStore,
};
use serde_json::{Value, json};

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
    core.register_specs(specs.clone(), projects.clone(), plans.clone())
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
    database: Database,
}

fn rules_harness() -> RulesHarness {
    let mut h = harness();
    let attachments = h._dir.path().join("attachments");
    register_issue_stack(&mut h.core, &h.database, &attachments);
    let core = Arc::new(h.core);
    RulesHarness {
        session: OperatorSession::new(core.clone()),
        core,
        database: h.database,
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
    core.register_specs(specs.clone(), projects.clone(), plans.clone())
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
        dependencies.clone(),
        projects.clone(),
        schedules,
        None,
    )
    .expect("lifecycle registers");
    core.register_dependencies(dependencies.clone(), tickets.clone(), projects.clone())
        .expect("dependency writes register");
    core.register_reassignment(tickets.clone(), projects.clone(), specs.clone())
        .expect("reassignment registers");
    core.register_profiles(
        Arc::new(kanban_storage::SqliteProfileStore::new(database)),
        tickets.clone(),
        projects.clone(),
    )
    .expect("profiles register");
    core.register_board(
        Arc::new(kanban_storage::SqliteInitiativeStore::new(database)),
        projects.clone(),
        plans,
        specs.clone(),
        tickets.clone(),
        Arc::new(kanban_storage::SqliteLaneStore::new(database)),
        Arc::new(kanban_storage::SqliteProfileStore::new(database)),
        None,
    )
    .expect("the board registers");
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
        database: h.database,
        _dir: h._dir,
    }
}

fn create_legacy_task(core: &Core, key: &str, title: &str) -> serde_json::Value {
    create_legacy_task_with_priority(core, key, title, "normal")
}

fn create_legacy_task_with_priority(
    core: &Core,
    key: &str,
    title: &str,
    priority: &str,
) -> serde_json::Value {
    core.command(
        "ticket.create",
        &json!({
            "mutation": mutation(0, key),
            "project_id": 1,
            "kind": "task",
            "priority": priority,
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

/// A description patch whose find and replace agree rewrites the
/// description to itself: version, history and updated_at stand still.
#[test]
fn identity_description_patches_change_nothing() {
    let h = rules_harness();
    let session = &h.session;

    let saved = save_issue(
        session,
        0,
        "patch-create",
        json!({
            "project_id": 1,
            "title": "Patched issue",
            "description": "Preserved wording",
            "state": "Active",
        }),
    );
    let before = read_issue(session, &saved.identifier);

    save_issue(
        session,
        before.version,
        "patch-identity",
        json!({
            "identifier": saved.identifier,
            "description_patch": "{\"find\":\"Preserved\",\"replace\":\"Preserved\"}",
        }),
    );
    let after = read_issue(session, &saved.identifier);
    assert_eq!(
        after.version, before.version,
        "a patch that rewrites the description to itself keeps the version"
    );
    assert_eq!(
        after.updated_at, before.updated_at,
        "an identity patch keeps updated_at"
    );
    assert_eq!(
        after.state_history.len(),
        before.state_history.len(),
        "an identity patch writes no history"
    );
    assert_eq!(
        after.description, before.description,
        "an identity patch leaves the description byte-for-byte"
    );
}

/// A legacy task reports its initial state through the legacy
/// fallback; saving that same state changes nothing, even though the
/// stored workflow id is still unset.
#[test]
fn saving_the_reported_state_of_a_legacy_task_changes_nothing() {
    let h = legacy_harness();
    let session = &h.session;
    let created = create_legacy_task(&h.core, "legacy-same-state-create", "Unmoved task");
    let identifier = format!("CORE-{}", created["number"].as_u64().expect("a number"));

    let before = read_issue(session, &identifier);
    assert_eq!(before.state, "Draft");

    save_issue(
        session,
        before.version,
        "legacy-same-state-save",
        json!({ "identifier": identifier, "state": "Draft" }),
    );
    let after = read_issue(session, &identifier);
    assert_eq!(
        after.version, before.version,
        "saving the reported state keeps the version"
    );
    assert_eq!(
        after.updated_at, before.updated_at,
        "saving the reported state keeps updated_at"
    );
    assert_eq!(
        after.state_history.len(),
        before.state_history.len(),
        "saving the reported state writes no history"
    );
    assert_eq!(after.state, "Draft");
}

/// Marking an issue a duplicate of a target it already names is a
/// no-op only while it still sits in Duplicate; once it has moved on,
/// re-marking the target moves it back.
#[test]
fn reapplying_a_moved_duplicate_target_returns_to_duplicate() {
    let h = rules_harness();
    let session = &h.session;

    let target = save_issue(
        session,
        0,
        "duplicate-target",
        json!({ "project_id": 1, "title": "Duplicate target" }),
    );
    let duplicate = save_issue(
        session,
        0,
        "duplicate-source",
        json!({ "project_id": 1, "title": "Duplicate source" }),
    );

    save_issue(
        session,
        issue_version(session, &duplicate.identifier),
        "duplicate-mark",
        json!({ "identifier": duplicate.identifier, "duplicate_of": target.identifier }),
    );
    let marked = read_issue(session, &duplicate.identifier);
    assert_eq!(marked.state, "Duplicate");
    assert!(marked.canceled_at.is_some());

    save_issue(
        session,
        marked.version,
        "duplicate-remark",
        json!({
            "identifier": duplicate.identifier,
            "duplicate_of": target.identifier.to_lowercase(),
        }),
    );
    let unchanged = read_issue(session, &duplicate.identifier);
    assert_eq!(
        unchanged.version, marked.version,
        "re-marking the held target while in Duplicate is a no-op"
    );
    assert_eq!(unchanged.state_history.len(), marked.state_history.len());

    save_issue(
        session,
        unchanged.version,
        "duplicate-done",
        json!({ "identifier": duplicate.identifier, "state": "Done" }),
    );
    let done = read_issue(session, &duplicate.identifier);
    assert_eq!(done.state, "Done");
    assert!(done.completed_at.is_some());

    save_issue(
        session,
        done.version,
        "duplicate-return",
        json!({ "identifier": duplicate.identifier, "duplicate_of": target.identifier }),
    );
    let reentered = read_issue(session, &duplicate.identifier);
    assert_eq!(
        reentered.state, "Duplicate",
        "re-marking the target after a move re-enters Duplicate"
    );
    assert!(
        reentered.completed_at.is_none(),
        "re-entering Duplicate drops the completed stamp"
    );
    assert!(reentered.canceled_at.is_some());
    let names = history_names(&reentered);
    assert!(
        names.ends_with(&["Done", "Duplicate"]),
        "the re-entry closes the Done row and opens a Duplicate row, saw {names:?}"
    );
    assert!(reentered.state_history.last().unwrap().ended_at.is_none());
}

/// `issue.save` with fields that must be refused, so tests can assert the
/// exact rejection instead of the panic-on-error happy helper.
fn try_save(
    session: &OperatorSession,
    version: u64,
    key: &str,
    fields: serde_json::Value,
) -> Result<Value, ApiError> {
    let mut object = serde_json::Map::new();
    object.insert("mutation".into(), mutation(version, key));
    if let Some(extra) = fields.as_object() {
        object.extend(extra.clone());
    }
    session.call("issue.save", &serde_json::Value::Object(object))
}

#[test]
fn calendar_due_dates_must_be_padded_and_trimmed() {
    let h = rules_harness();
    let session = &h.session;
    let saved = save_issue(
        session,
        0,
        "strict-dates-host",
        json!({ "project_id": 1, "title": "Strict dates" }),
    );

    for (index, raw) in [
        "2026-1-5",
        " 2026-11-05",
        "2026-11-5 ",
        "2026-99-99",
        "2026-02-30",
        "2026-11-05T00:00:00Z",
    ]
    .into_iter()
    .enumerate()
    {
        let error = try_save(
            session,
            issue_version(session, &saved.identifier),
            &format!("strict-dates-{index}"),
            json!({ "identifier": saved.identifier, "due_date": raw }),
        )
        .expect_err("a loose calendar date is refused");
        assert_eq!(error.code, ErrorCode::InvalidRequest, "{raw}");
        assert_eq!(error.field.as_deref(), Some("due_date"), "{raw}");
    }

    save_issue(
        session,
        issue_version(session, &saved.identifier),
        "strict-dates-exact",
        json!({ "identifier": saved.identifier, "due_date": "2026-11-05" }),
    );
    let read = read_issue(session, &saved.identifier);
    assert_eq!(read.due_date.as_deref(), Some("2026-11-05"));
}

#[test]
fn milestone_target_dates_follow_the_calendar_rule() {
    let h = rules_harness();
    let session = &h.session;

    for (index, raw) in ["2026-99-99", "2026-1-5", " 2026-11-05"]
        .into_iter()
        .enumerate()
    {
        let error = session
            .call(
                "issue.create_milestone",
                &json!({
                    "mutation": mutation(0, format!("loose-target-{index}")),
                    "project_id": 1,
                    "name": "Dated milestone",
                    "target_date": raw,
                }),
            )
            .expect_err("an impossible target date is refused");
        assert_eq!(error.code, ErrorCode::InvalidRequest, "{raw}");
        assert_eq!(error.field.as_deref(), Some("target_date"), "{raw}");
    }

    let created = session
        .call(
            "issue.create_milestone",
            &json!({
                "mutation": mutation(0, "target-date-exact"),
                "project_id": 1,
                "name": "Dated milestone",
                "target_date": "2026-12-24",
            }),
        )
        .expect("a zero-padded target date is accepted");
    assert_eq!(created["target_date"], json!("2026-12-24"));

    let undated = session
        .call(
            "issue.create_milestone",
            &json!({
                "mutation": mutation(0, "target-date-absent"),
                "project_id": 1,
                "name": "Undated milestone",
            }),
        )
        .expect("a milestone without a target date is accepted");
    assert_eq!(undated["target_date"], json!(null));
}

#[test]
fn an_issue_cannot_reference_itself_in_relations() {
    let h = rules_harness();
    let session = &h.session;
    let saved = save_issue(
        session,
        0,
        "self-reference-host",
        json!({ "project_id": 1, "title": "Self references" }),
    );

    for (field, key) in [
        ("add_blocks", "self-blocks"),
        ("add_related", "self-related"),
        ("add_blocked_by", "self-blocked-by"),
    ] {
        let error = try_save(
            session,
            issue_version(session, &saved.identifier),
            key,
            json!({ "identifier": saved.identifier, field: [saved.identifier.clone()] }),
        )
        .expect_err("a self reference is refused");
        assert_eq!(error.code, ErrorCode::InvalidRequest, "{field}");
        assert_eq!(error.field.as_deref(), Some(field), "{field}");
    }

    let read = read_issue(session, &saved.identifier);
    assert!(
        read.relations.is_empty(),
        "no self relation row may be written"
    );
}

#[test]
fn blank_title_updates_are_refused() {
    let h = rules_harness();
    let session = &h.session;
    let saved = save_issue(
        session,
        0,
        "blank-title-host",
        json!({ "project_id": 1, "title": "Kept title" }),
    );

    for (index, title) in ["", "   "].into_iter().enumerate() {
        let error = try_save(
            session,
            issue_version(session, &saved.identifier),
            &format!("blank-title-{index}"),
            json!({ "identifier": saved.identifier, "title": title }),
        )
        .expect_err("a blank title is refused");
        assert_eq!(error.code, ErrorCode::InvalidRequest, "{title:?}");
        assert_eq!(error.field.as_deref(), Some("title"), "{title:?}");
    }

    let read = read_issue(session, &saved.identifier);
    assert_eq!(read.title, "Kept title", "the title is unchanged");

    save_issue(
        session,
        issue_version(session, &saved.identifier),
        "blank-title-replacement",
        json!({ "identifier": saved.identifier, "title": "New title" }),
    );
    assert_eq!(read_issue(session, &saved.identifier).title, "New title");
}
#[test]
fn blank_titles_are_refused_when_creating() {
    let h = rules_harness();
    let session = &h.session;

    for (index, title) in ["", " ", "\t"].into_iter().enumerate() {
        let error = try_save(
            session,
            0,
            &format!("create-blank-title-{index}"),
            json!({ "project_id": 1, "title": title }),
        )
        .expect_err("a blank title cannot create an issue");
        assert_eq!(error.code, ErrorCode::InvalidRequest, "{title:?}");
        assert_eq!(
            error.field.as_deref(),
            Some("title"),
            "{title:?}: {}",
            error.message
        );
    }

    let missing = try_save(
        session,
        0,
        "create-without-title",
        json!({ "project_id": 1 }),
    )
    .expect_err("an absent title cannot create an issue");
    assert_eq!(missing.code, ErrorCode::InvalidRequest);
    assert_eq!(missing.field.as_deref(), Some("title"));

    let padded = save_issue(
        session,
        0,
        "create-padded-title",
        json!({ "project_id": 1, "title": "  Padded title  " }),
    );
    assert_eq!(
        read_issue(session, &padded.identifier).title,
        "  Padded title  ",
        "a title with text keeps its surrounding spaces"
    );
}

#[test]
fn one_label_per_single_select_group_in_one_call() {
    let h = rules_harness();
    let session = &h.session;
    for (name, key) in [("simple", "tier-simple"), ("standard", "tier-standard")] {
        session
            .call(
                "issue.create_label",
                &json!({
                    "mutation": mutation(0, key),
                    "project_id": 1,
                    "name": name,
                    "color": "#336699",
                    "group": "Model tier",
                    "group_single_select": true,
                }),
            )
            .expect("the tier label is created");
    }
    session
        .call(
            "issue.create_label",
            &json!({
                "mutation": mutation(0, "area-label"),
                "project_id": 1,
                "name": "web",
                "color": "#336699",
            }),
        )
        .expect("an ungrouped label is created");
    session
        .call(
            "issue.create_label",
            &json!({
                "mutation": mutation(0, "stage-label"),
                "project_id": 1,
                "name": "brief",
                "color": "#336699",
                "group": "Stage",
                "group_single_select": false,
            }),
        )
        .expect("a multi-select group label is created");
    session
        .call(
            "issue.create_label",
            &json!({
                "mutation": mutation(0, "stage-label-two"),
                "project_id": 1,
                "name": "pitch",
                "color": "#336699",
                "group": "Stage",
                "group_single_select": false,
            }),
        )
        .expect("the second multi-select group label is created");

    let saved = save_issue(
        session,
        0,
        "single-select-host",
        json!({ "project_id": 1, "title": "One tier only" }),
    );

    let error = try_save(
        session,
        issue_version(session, &saved.identifier),
        "single-select-conflict",
        json!({
            "identifier": saved.identifier,
            "add_labels": ["simple", "standard"],
        }),
    )
    .expect_err("two labels from one single-select group are refused");
    assert_eq!(error.code, ErrorCode::InvalidRequest);
    assert_eq!(error.field.as_deref(), Some("add_labels"));
    assert!(
        error.message.contains("simple") && error.message.contains("standard"),
        "the message names both labels: {}",
        error.message
    );
    assert!(
        read_issue(session, &saved.identifier).labels.is_empty(),
        "the refused call adds neither label"
    );

    save_issue(
        session,
        issue_version(session, &saved.identifier),
        "single-select-first",
        json!({ "identifier": saved.identifier, "add_labels": ["simple"] }),
    );
    save_issue(
        session,
        issue_version(session, &saved.identifier),
        "single-select-replace",
        json!({ "identifier": saved.identifier, "add_labels": ["standard"] }),
    );
    let read = read_issue(session, &saved.identifier);
    assert_eq!(
        read.labels,
        vec!["standard".to_owned()],
        "a later single-select add still replaces the held label"
    );

    save_issue(
        session,
        issue_version(session, &saved.identifier),
        "multi-select-both",
        json!({
            "identifier": saved.identifier,
            "add_labels": ["brief", "pitch", "web"],
        }),
    );
    let read = read_issue(session, &saved.identifier);
    assert_eq!(
        read.labels,
        vec![
            "brief".to_owned(),
            "pitch".to_owned(),
            "standard".to_owned(),
            "web".to_owned()
        ],
        "multi-select groups and ungrouped labels keep every label"
    );
}
#[test]
fn single_select_conflicts_surface_across_a_whole_batch() {
    let h = rules_harness();
    let session = &h.session;
    for (name, key) in [("small", "batch-tier-small"), ("large", "batch-tier-large")] {
        session
            .call(
                "issue.create_label",
                &json!({
                    "mutation": mutation(0, key),
                    "project_id": 1,
                    "name": name,
                    "color": "#336699",
                    "group": "Model tier",
                    "group_single_select": true,
                }),
            )
            .expect("the tier label is created");
    }
    session
        .call(
            "issue.create_label",
            &json!({
                "mutation": mutation(0, "batch-mode-agent"),
                "project_id": 1,
                "name": "agent",
                "color": "#336699",
                "group": "Run mode",
                "group_single_select": true,
            }),
        )
        .expect("the mode label is created");

    let saved = save_issue(
        session,
        0,
        "batch-single-select-host",
        json!({ "project_id": 1, "title": "Interleaved tiers" }),
    );
    let version = issue_version(session, &saved.identifier);

    let error = try_save(
        session,
        version,
        "batch-interleaved-conflict",
        json!({
            "identifier": saved.identifier,
            "add_labels": ["small", "agent", "large"],
        }),
    )
    .expect_err("two tier labels separated by a mode label are refused");
    assert_eq!(error.code, ErrorCode::InvalidRequest);
    assert_eq!(error.field.as_deref(), Some("add_labels"));
    assert!(
        error.message.contains("small") && error.message.contains("large"),
        "the message names both tier labels: {}",
        error.message
    );
    let untouched = read_issue(session, &saved.identifier);
    assert!(
        untouched.labels.is_empty(),
        "the refused call adds neither label"
    );
    assert_eq!(
        untouched.version, version,
        "the refused call writes nothing"
    );

    save_issue(
        session,
        version,
        "batch-distinct-groups",
        json!({
            "identifier": saved.identifier,
            "add_labels": ["small", "agent"],
        }),
    );
    let read = read_issue(session, &saved.identifier);
    assert!(
        read.labels.contains(&"small".to_owned()) && read.labels.contains(&"agent".to_owned()),
        "both distinct-group labels are held: {:?}",
        read.labels
    );

    save_issue(
        session,
        read.version,
        "batch-repeated-label",
        json!({
            "identifier": saved.identifier,
            "add_labels": ["agent", "agent"],
        }),
    );
    let repeated = read_issue(session, &saved.identifier);
    assert_eq!(
        repeated
            .labels
            .iter()
            .filter(|label| label.as_str() == "agent")
            .count(),
        1,
        "the repeated label is held once: {:?}",
        repeated.labels
    );
}

#[test]
fn created_issues_without_a_priority_carry_no_priority() {
    let h = rules_harness();
    let session = &h.session;

    let unset = save_issue(
        session,
        0,
        "priority-unset",
        json!({ "project_id": 1, "title": "No priority given" }),
    );
    assert_eq!(
        read_issue(session, &unset.identifier).priority,
        0,
        "a new issue without a priority starts at No priority"
    );

    let explicit = save_issue(
        session,
        0,
        "priority-explicit",
        json!({ "project_id": 1, "title": "Explicit priority", "priority": 2 }),
    );
    assert_eq!(read_issue(session, &explicit.identifier).priority, 2);
}

#[test]
fn retiring_an_already_retired_label_says_so() {
    let h = rules_harness();
    let session = &h.session;
    session
        .call(
            "issue.create_label",
            &json!({
                "mutation": mutation(0, "retire-once-label"),
                "project_id": 1,
                "name": "old-label",
                "color": "#336699",
            }),
        )
        .expect("the label is created");

    session
        .call(
            "issue.retire_label",
            &json!({
                "mutation": mutation(0, "retire-once"),
                "project_id": 1,
                "name": "old-label",
            }),
        )
        .expect("the first retirement applies");

    let again = session
        .call(
            "issue.retire_label",
            &json!({
                "mutation": mutation(0, "retire-twice"),
                "project_id": 1,
                "name": "old-label",
            }),
        )
        .expect_err("retiring an already-retired label is refused");
    assert_eq!(again.code, ErrorCode::InvalidRequest);
    assert_eq!(again.field.as_deref(), Some("name"));
    assert!(
        again.message.contains("already retired"),
        "the message says the label is already retired: {}",
        again.message
    );

    let unknown = session
        .call(
            "issue.retire_label",
            &json!({
                "mutation": mutation(0, "retire-unknown"),
                "project_id": 1,
                "name": "ghost-label",
            }),
        )
        .expect_err("an unknown label is still not found");
    assert_eq!(unknown.code, ErrorCode::NotFound);
}

/// Wait until the SQLite clock's millisecond tick passes `earlier`,
/// so a stamp taken after the wait is strictly newer.
fn wait_for_clock_past(h: &RulesHarness, earlier: &str) -> String {
    for _ in 0..200 {
        let now: String = h
            .database
            .connection()
            .query_row("SELECT strftime('%Y-%m-%dT%H:%M:%fZ', 'now')", [], |row| {
                row.get(0)
            })
            .expect("the clock serves");
        if now.as_str() > earlier {
            return now;
        }
        std::thread::sleep(std::time::Duration::from_millis(3));
    }
    panic!("the SQLite clock never advanced past {earlier}");
}

#[test]
fn legacy_state_moves_stamp_the_issue_update_time() {
    let h = legacy_harness();
    let session = &h.session;
    let created = create_legacy_task(&h.core, "legacy-stamp-create", "Legacy stamped task");
    let ticket_id = created["id"].as_u64().expect("a ticket id");
    let identifier = format!("CORE-{}", created["number"].as_u64().expect("a number"));

    save_issue(
        session,
        created["version"].as_u64().expect("a version"),
        "legacy-stamp-ready",
        json!({ "identifier": identifier, "state": "Ready" }),
    );
    let before = read_issue(session, &identifier);
    let moved_at = wait_for_clock_past(&h, &before.updated_at);

    h.core
        .command(
            "ticket.cancel",
            &json!({
                "mutation": mutation(issue_version(session, &identifier), "legacy-stamp-cancel"),
                "ticket_id": ticket_id,
            }),
        )
        .expect("the legacy cancel lands");

    let moved = read_issue(session, &identifier);
    assert_eq!(moved.state, "Cancelled");
    assert!(
        moved.updated_at >= moved_at,
        "the legacy move stamps updated_at ({} may share the sampled millisecond {})",
        moved.updated_at,
        moved_at
    );
    assert!(
        moved.updated_at > before.updated_at,
        "the stamped update time advances past the pre-move update time ({} must pass {})",
        moved.updated_at,
        before.updated_at
    );
    assert_eq!(
        moved.version,
        before.version + 1,
        "the move keeps its own single version increment"
    );

    let visible: Vec<String> = session
        .call(
            "issue.list",
            &json!({ "project_id": 1, "updated_since": moved_at }),
        )
        .expect("the listing serves")["issues"]
        .as_array()
        .expect("an issues array")
        .iter()
        .map(|issue| {
            issue["identifier"]
                .as_str()
                .expect("an identifier")
                .to_owned()
        })
        .collect();
    assert!(
        visible.contains(&identifier),
        "updated_since sees the legacy-moved issue: {visible:?}"
    );
}

#[test]
fn legacy_child_closing_closes_the_parent_when_enabled() {
    let h = legacy_harness();
    let session = &h.session;
    let parent = save_issue(
        session,
        0,
        "auto-close-legacy-parent",
        json!({ "project_id": 1, "title": "Parent awaiting the child" }),
    );
    let child = save_issue(
        session,
        0,
        "auto-close-legacy-child",
        json!({
            "project_id": 1,
            "title": "Child closed the legacy way",
            "parent_id": parent.ticket_id.to_string(),
        }),
    );
    session
        .call(
            "issue.update_project_settings",
            &json!({
                "mutation": mutation(0, "auto-close-legacy-enable"),
                "project_id": 1,
                "auto_close_sub_issues": true,
            }),
        )
        .expect("auto-close is enabled");
    save_issue(
        session,
        issue_version(session, &child.identifier),
        "auto-close-legacy-ready",
        json!({ "identifier": child.identifier, "state": "Ready" }),
    );

    h.core
        .command(
            "ticket.cancel",
            &json!({
                "mutation": mutation(issue_version(session, &child.identifier), "auto-close-legacy-cancel"),
                "ticket_id": child.ticket_id,
            }),
        )
        .expect("the legacy cancel lands");

    let moved_child = read_issue(session, &child.identifier);
    assert_eq!(moved_child.state, "Cancelled");
    let closed = read_issue(session, &parent.identifier);
    assert_eq!(
        closed.state, "Done",
        "the parent closes with the legacy-closed child, in the same write"
    );
    assert!(closed.completed_at.is_some());
}

/// The identifier a legacy creation mints for its ticket.
fn legacy_identifier(created: &Value) -> String {
    format!("CORE-{}", created["number"].as_u64().expect("a number"))
}

/// A legacy Task moved to Ready over the operator surface, so its
/// workflow state is already seeded the way every issued ticket's
/// is: the sweep then measures what a later legacy write alone does.
fn seeded_task(h: &RulesHarness, key: &str, title: &str) -> (String, u64, u64) {
    let created = create_legacy_task(&h.core, key, title);
    let identifier = legacy_identifier(&created);
    let ticket_id = created["id"].as_u64().expect("a ticket id");
    let before = read_issue(&h.session, &identifier);
    save_issue(
        &h.session,
        before.version,
        &format!("{key}-ready"),
        json!({ "identifier": identifier, "state": "Ready" }),
    );
    let seeded = read_issue(&h.session, &identifier);
    (identifier, ticket_id, seeded.version)
}

#[test]
fn a_legacy_created_task_opens_its_initial_state_row() {
    let h = legacy_harness();
    let created = create_legacy_task(&h.core, "legacy-create-history", "Legacy created task");
    let identifier = legacy_identifier(&created);

    let record = read_issue(&h.session, &identifier);
    assert_eq!(record.state, "Draft", "a legacy creation starts in Draft");
    let names = history_names(&record);
    assert_eq!(
        names,
        vec!["Draft"],
        "the legacy creation records the initial state's open row, saw {names:?}"
    );
    assert!(
        record.state_history[0].ended_at.is_none(),
        "the creation row stays open until a move closes it"
    );

    let ticket_id = created["id"].as_u64().expect("a ticket id");
    h.core
        .command(
            "ticket.park",
            &json!({
                "mutation": mutation(record.version, "legacy-create-history-park"),
                "ticket_id": ticket_id,
            }),
        )
        .expect("the legacy park lands");
    let parked = read_issue(&h.session, &identifier);
    let names = history_names(&parked);
    assert_eq!(
        names,
        vec!["Draft", "Parked"],
        "the later move closes the creation row instead of replacing it, saw {names:?}"
    );
    assert!(parked.state_history[0].ended_at.is_some());
    assert!(parked.state_history[1].ended_at.is_none());
}

#[test]
fn legacy_creation_maps_priorities_the_way_the_migration_does() {
    for (wire, linear) in [("urgent", 1u8), ("high", 2), ("normal", 3), ("low", 4)] {
        let h = legacy_harness();
        let created = create_legacy_task_with_priority(
            &h.core,
            &format!("legacy-create-{wire}"),
            "Priority mapping task",
            wire,
        );
        let record = read_issue(&h.session, &legacy_identifier(&created));
        assert_eq!(
            record.priority, linear,
            "legacy priority {wire} maps to Linear priority {linear}"
        );
    }
}

#[test]
fn legacy_prioritise_moves_the_issue_priority() {
    let h = legacy_harness();
    let created =
        create_legacy_task_with_priority(&h.core, "legacy-prioritise", "Prioritised task", "high");
    let identifier = legacy_identifier(&created);
    let ticket_id = created["id"].as_u64().expect("a ticket id");
    assert_eq!(read_issue(&h.session, &identifier).priority, 2);

    h.core
        .command(
            "ticket.prioritise",
            &json!({
                "mutation": mutation(read_issue(&h.session, &identifier).version, "legacy-prioritise-urgent"),
                "ticket_id": ticket_id,
                "priority": "urgent",
            }),
        )
        .expect("the legacy prioritise lands");
    let urgent = read_issue(&h.session, &identifier);
    assert_eq!(urgent.priority, 1, "prioritise urgent maps to 1");
    assert!(
        urgent.version > created["version"].as_u64().expect("a version"),
        "the legacy move still bumps the version"
    );

    h.core
        .command(
            "ticket.prioritise",
            &json!({
                "mutation": mutation(read_issue(&h.session, &identifier).version, "legacy-prioritise-low"),
                "ticket_id": ticket_id,
                "priority": "low",
            }),
        )
        .expect("the second legacy prioritise lands");
    assert_eq!(
        read_issue(&h.session, &identifier).priority,
        4,
        "prioritise low maps to 4"
    );
}

#[test]
fn a_legacy_edit_stamps_the_issue_update_time() {
    let h = legacy_harness();
    let (identifier, ticket_id, version) = seeded_task(&h, "legacy-edit-stamp", "Edited task");
    h.core
        .command(
            "ticket.park",
            &json!({
                "mutation": mutation(version, "legacy-edit-stamp-park"),
                "ticket_id": ticket_id,
            }),
        )
        .expect("the park seeds the workflow state");
    let before = read_issue(&h.session, &identifier);
    let stamp = wait_for_clock_past(&h, &before.updated_at);

    h.core
        .command(
            "ticket.edit",
            &json!({
                "mutation": mutation(before.version, "legacy-edit-stamp-edit"),
                "ticket_id": ticket_id,
                "title": "Retitled task",
            }),
        )
        .expect("the legacy edit lands");
    let edited = read_issue(&h.session, &identifier);
    assert_eq!(edited.title, "Retitled task");
    assert_eq!(edited.state, "Parked", "the edit moves no state");
    assert!(
        edited.updated_at >= stamp,
        "a same-state legacy edit stamps updated_at ({} may share the sampled millisecond {})",
        edited.updated_at,
        stamp
    );
    assert!(
        edited.updated_at > before.updated_at,
        "the edit advances the update time past the pre-edit stamp ({} must pass {})",
        edited.updated_at,
        before.updated_at
    );
}

/// Every legacy write command that lands through the shared ticket
/// row move stamps the issue's `updated_at` once the workflow state
/// is seeded: the sweep covers each command family — the named human
/// commands, the drag, profile assignment, bug qualification and
/// facts, and the audited emergency override.
#[test]
fn every_legacy_write_stamps_the_issue_update_time() {
    let h = legacy_harness();

    let stamps = |command: &str, key: &str, payload: fn(u64, u64) -> serde_json::Value| {
        let (identifier, ticket_id, version) = seeded_task(&h, key, "Sweep task");
        let _ = version;
        let before = read_issue(&h.session, &identifier);
        let clock = wait_for_clock_past(&h, &before.updated_at);
        let payload = payload(ticket_id, before.version);
        h.core
            .command(command, &payload)
            .unwrap_or_else(|error| panic!("the legacy {command} lands: {error:?}"));
        let after = read_issue(&h.session, &identifier);
        assert!(
            after.updated_at >= clock,
            "the legacy {command} stamps updated_at ({} may share the sampled millisecond {})",
            after.updated_at,
            clock
        );
        assert!(
            after.updated_at > before.updated_at,
            "the legacy {command} advances the update time past {} ",
            before.updated_at
        );
        after
    };

    stamps(
        "ticket.park",
        "sweep-park",
        |id, version| json!({"mutation": mutation(version, "sweep-park-move"), "ticket_id": id}),
    );
    stamps("ticket.transition", "sweep-transition", |id, version| {
        json!({
            "mutation": mutation(version, "sweep-transition-move"),
            "ticket_id": id,
            "to": "active"
        })
    });
    stamps("ticket.prioritise", "sweep-prioritise", |id, version| {
        json!({
            "mutation": mutation(version, "sweep-prioritise-move"),
            "ticket_id": id,
            "priority": "low"
        })
    });
    stamps("ticket.edit", "sweep-edit", |id, version| {
        json!({
            "mutation": mutation(version, "sweep-edit-move"),
            "ticket_id": id,
            "title": "Sweep retitled"
        })
    });
    stamps(
        "ticket.assign",
        "sweep-assign",
        |id, version| json!({"mutation": mutation(version, "sweep-assign-move"), "ticket_id": id, "profile": "standard"}),
    );

    // Scheduling leaves Draft, so its ticket is created over the
    // operator surface, whose Draft row is seeded at creation.
    {
        let saved = save_issue(
            &h.session,
            0,
            "sweep-schedule",
            json!({ "project_id": 1, "title": "Scheduled sweep task" }),
        );
        let identifier = saved.identifier.clone();
        let ticket_id = saved.ticket_id;
        let before = read_issue(&h.session, &identifier);
        assert_eq!(before.state, "Draft");
        let clock = wait_for_clock_past(&h, &before.updated_at);
        h.core
            .command(
                "ticket.schedule",
                &json!({
                    "mutation": mutation(before.version, "sweep-schedule-move"),
                    "ticket_id": ticket_id,
                    "cron": "*/15 * * * *",
                    "timezone": "UTC",
                    "after": "2026-09-08T09:00:00Z",
                    "profile": "standard",
                }),
            )
            .expect("the legacy schedule lands");
        let after = read_issue(&h.session, &identifier);
        assert_eq!(after.state, "Scheduled");
        assert!(
            after.updated_at >= clock,
            "the legacy schedule stamps updated_at, stayed at {}",
            after.updated_at
        );
    }

    // Unpark needs a parked ticket first.
    {
        let (identifier, ticket_id, version) = seeded_task(&h, "sweep-unpark", "Unparked task");
        h.core
            .command(
                "ticket.park",
                &json!({
                    "mutation": mutation(version, "sweep-unpark-park"),
                    "ticket_id": ticket_id,
                }),
            )
            .expect("the park lands");
        let before = read_issue(&h.session, &identifier);
        let clock = wait_for_clock_past(&h, &before.updated_at);
        h.core
            .command(
                "ticket.unpark",
                &json!({
                    "mutation": mutation(before.version, "sweep-unpark-move"),
                    "ticket_id": ticket_id,
                }),
            )
            .expect("the unpark lands");
        let after = read_issue(&h.session, &identifier);
        assert!(
            after.updated_at >= clock,
            "the legacy unpark stamps updated_at, stayed at {}",
            after.updated_at
        );
    }

    // The review decision: reach In Review, then approve it.
    {
        let (identifier, ticket_id, version) = seeded_task(&h, "sweep-review", "Review sweep task");
        let _ = version;
        let in_review = read_issue(&h.session, &identifier);
        save_issue(
            &h.session,
            in_review.version,
            "sweep-review-in-review",
            json!({"identifier": identifier, "state": "In Review"}),
        );
        let before = read_issue(&h.session, &identifier);
        let clock = wait_for_clock_past(&h, &before.updated_at);
        h.core
            .command(
                "ticket.review",
                &json!({
                    "mutation": mutation(before.version, "sweep-review-move"),
                    "ticket_id": ticket_id,
                    "decision": "approve",
                }),
            )
            .expect("the legacy review lands");
        let after = read_issue(&h.session, &identifier);
        assert_eq!(after.state, "Approved");
        assert!(
            after.updated_at >= clock,
            "the legacy review stamps updated_at, stayed at {}",
            after.updated_at
        );
    }

    // The audited emergency override moves past terminal states.
    {
        let (identifier, ticket_id, version) =
            seeded_task(&h, "sweep-override", "Override sweep task");
        h.core
            .command(
                "ticket.cancel",
                &json!({
                    "mutation": mutation(version, "sweep-override-cancel"),
                    "ticket_id": ticket_id,
                }),
            )
            .expect("the legacy cancel lands");
        let cancelled = read_issue(&h.session, &identifier);
        let clock = wait_for_clock_past(&h, &cancelled.updated_at);
        h.core
            .command(
                "ticket.emergency.override",
                &json!({
                    "mutation": mutation(cancelled.version, "sweep-override-move"),
                    "ticket_id": ticket_id,
                    "to": "done",
                    "who": "operator",
                    "why": "the sweep reaches every legacy write",
                }),
            )
            .expect("the override lands");
        let after = read_issue(&h.session, &identifier);
        assert_eq!(after.state, "Done");
        assert!(
            after.updated_at >= clock,
            "the emergency override stamps updated_at, stayed at {}",
            after.updated_at
        );
    }

    // Bug qualification and facts: the bug-only legacy writes.
    {
        let bug = h
            .core
            .command(
                "ticket.create",
                &json!({
                    "mutation": mutation(0, "sweep-bug"),
                    "project_id": 1,
                    "kind": "bug",
                    "priority": "normal",
                    "title": "Sweep bug",
                    "actual_behaviour": "The sweep observes the bug writes.",
                    "reporter_evidence": "The sweep transcript.",
                }),
            )
            .expect("the legacy Bug lands");
        let identifier = legacy_identifier(&bug);
        let ticket_id = bug["id"].as_u64().expect("a ticket id");
        let draft = read_issue(&h.session, &identifier);
        save_issue(
            &h.session,
            draft.version,
            "sweep-bug-ready",
            json!({"identifier": identifier, "state": "Ready"}),
        );
        let before = read_issue(&h.session, &identifier);
        let clock = wait_for_clock_past(&h, &before.updated_at);
        h.core
            .command(
                "ticket.bug.qualify",
                &json!({
                    "mutation": mutation(before.version, "sweep-bug-qualify"),
                    "ticket_id": ticket_id,
                    "qualification": {
                        "expected_behaviour": "The sweep bug qualifies.",
                        "reproduction": "Run the sweep.",
                        "environment": "macOS 26, disposable SQLite.",
                        "severity": "critical",
                        "frequency": "Every run.",
                        "affected_scope": "The sweep.",
                        "risk": "None.",
                        "criteria": [{
                            "outcome": "The qualification lands.",
                            "stories": ["CORE-S1-US1"]
                        }],
                        "verification_steps": [{ "command": "cargo test" }]
                    }
                }),
            )
            .expect("the legacy qualification lands");
        let qualified = read_issue(&h.session, &identifier);
        assert!(
            qualified.updated_at >= clock,
            "the legacy qualification stamps updated_at, stayed at {}",
            qualified.updated_at
        );
        let clock = wait_for_clock_past(&h, &qualified.updated_at);
        h.core
            .command(
                "ticket.bug.facts",
                &json!({
                    "mutation": mutation(qualified.version, "sweep-bug-facts"),
                    "ticket_id": ticket_id,
                    "external_references": [],
                    "occurrence_snapshots": [],
                    "evidence_ids": [],
                }),
            )
            .expect("the legacy bug facts land");
        let after = read_issue(&h.session, &identifier);
        assert!(
            after.updated_at >= clock,
            "the legacy bug facts stamp updated_at, stayed at {}",
            after.updated_at
        );
    }

    // Dependency and blocker writes move the waiting ticket's
    // version, so its update stamp moves with them: the relation a
    // legacy write changes is a change the issue model must see.
    {
        let (identifier, ticket_id, version) =
            seeded_task(&h, "sweep-dependency", "Dependency sweep task");
        let _ = version;
        let blocker = create_legacy_task(&h.core, "sweep-dependency-blocker", "Blocking task");
        let blocker_id = blocker["id"].as_u64().expect("a ticket id");
        let before = read_issue(&h.session, &identifier);
        let clock = wait_for_clock_past(&h, &before.updated_at);
        h.core
            .command(
                "ticket.dependency.add",
                &json!({
                    "mutation": mutation(before.version, "sweep-dependency-add"),
                    "from_ticket": blocker_id,
                    "to_ticket": ticket_id,
                }),
            )
            .expect("the legacy dependency add lands");
        let after_add = read_issue(&h.session, &identifier);
        assert_eq!(
            after_add.version,
            before.version + 1,
            "the dependency add moves the waiting version once"
        );
        assert!(
            after_add.updated_at >= clock,
            "the legacy dependency add stamps updated_at, stayed at {}",
            after_add.updated_at
        );
        assert_eq!(
            after_add.state_history, before.state_history,
            "a dependency add changes no workflow state"
        );

        let before = read_issue(&h.session, &identifier);
        let clock = wait_for_clock_past(&h, &before.updated_at);
        h.core
            .command(
                "ticket.dependency.remove",
                &json!({
                    "mutation": mutation(before.version, "sweep-dependency-remove"),
                    "from_ticket": blocker_id,
                    "to_ticket": ticket_id,
                }),
            )
            .expect("the legacy dependency remove lands");
        let after = read_issue(&h.session, &identifier);
        assert!(
            after.updated_at >= clock,
            "the legacy dependency remove stamps updated_at, stayed at {}",
            after.updated_at
        );
    }

    {
        let (identifier, ticket_id, version) =
            seeded_task(&h, "sweep-blocker", "Blocker sweep task");
        let _ = version;
        let before = read_issue(&h.session, &identifier);
        let clock = wait_for_clock_past(&h, &before.updated_at);
        let added = h
            .core
            .command(
                "ticket.blocker.add",
                &json!({
                    "mutation": mutation(before.version, "sweep-blocker-add"),
                    "ticket_id": ticket_id,
                    "description": "The sweep waits on an external blocker",
                }),
            )
            .expect("the legacy blocker add lands");
        let after_add = read_issue(&h.session, &identifier);
        assert!(
            after_add.updated_at >= clock,
            "the legacy blocker add stamps updated_at, stayed at {}",
            after_add.updated_at
        );
        let blocker_id = added["blockers"][0]["id"]
            .as_u64()
            .expect("the recorded blocker carries its id");
        let before = read_issue(&h.session, &identifier);
        let clock = wait_for_clock_past(&h, &before.updated_at);
        h.core
            .command(
                "ticket.blocker.remove",
                &json!({
                    "mutation": mutation(before.version, "sweep-blocker-remove"),
                    "ticket_id": ticket_id,
                    "blocker_id": blocker_id,
                }),
            )
            .expect("the legacy blocker remove lands");
        let after = read_issue(&h.session, &identifier);
        assert!(
            after.updated_at >= clock,
            "the legacy blocker remove stamps updated_at, stayed at {}",
            after.updated_at
        );
    }
}

#[test]
fn a_legacy_reassignment_seeds_the_replacement_issue() {
    let h = legacy_harness();
    let created = create_legacy_task_with_priority(
        &h.core,
        "legacy-reassign-seed",
        "Reassigned task",
        "high",
    );
    let identifier = legacy_identifier(&created);
    let ticket_id = created["id"].as_u64().expect("a ticket id");
    let ready = read_issue(&h.session, &identifier);

    let replacement = h
        .core
        .command(
            "ticket.reassign",
            &json!({
                "mutation": mutation(ready.version, "legacy-reassign-seed-move"),
                "ticket_id": ticket_id,
                "kind": "task",
                "priority": "urgent",
                "title": "Replacement task",
                "subtype": "operational",
                "mode": "human",
                "completion": ["Checked"],
            }),
        )
        .expect("the reassignment lands");
    let replacement_key = format!("CORE-{}", replacement["number"].as_u64().expect("a number"));

    let record = read_issue(&h.session, &replacement_key);
    assert_eq!(record.state, "Draft", "the replacement starts in Draft");
    assert_eq!(
        record.priority, 1,
        "the replacement carries the requested urgent priority"
    );
    let names = history_names(&record);
    assert_eq!(
        names,
        vec!["Draft"],
        "the replacement opens its own initial state row, saw {names:?}"
    );
    assert!(record.state_history[0].ended_at.is_none());
}

#[test]
fn a_replacement_carries_no_due_date_until_one_is_set() {
    let h = legacy_harness();
    let created = create_legacy_task(&h.core, "legacy-reassign-date", "Dated task");
    let identifier = legacy_identifier(&created);
    let ticket_id = created["id"].as_u64().expect("a ticket id");

    // The original carries a real deadline far inside the calendar,
    // so the none marker cannot stand in for it on either row.
    let dated = read_issue(&h.session, &identifier);
    save_issue(
        &h.session,
        dated.version,
        "legacy-reassign-date-set",
        json!({ "identifier": identifier, "due_date": "2030-05-05" }),
    );
    assert_eq!(
        read_issue(&h.session, &identifier).due_date,
        Some("2030-05-05".to_owned())
    );

    let before = read_issue(&h.session, &identifier);
    let replacement = h
        .core
        .command(
            "ticket.reassign",
            &json!({
                "mutation": mutation(before.version, "legacy-reassign-date-move"),
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
    let replacement_key = format!("CORE-{}", replacement["number"].as_u64().expect("a number"));

    // A replacement is a new body, not a copy of the original's
    // dates: it starts with no deadline at all, and the retired
    // sentinel never reads back as a real 2099-12-31 deadline.
    let record = read_issue(&h.session, &replacement_key);
    assert_eq!(
        record.due_date, None,
        "the replacement starts without a deadline, saw {:?}",
        record.due_date
    );

    // The replacement accepts and keeps a real deadline of its own.
    save_issue(
        &h.session,
        record.version,
        "legacy-reassign-date-replacement",
        json!({ "identifier": replacement_key, "due_date": "2099-12-31" }),
    );
    assert_eq!(
        read_issue(&h.session, &replacement_key).due_date,
        Some("2099-12-31".to_owned()),
        "the farthest real calendar date round-trips on the replacement"
    );
}

#[test]
fn label_saves_that_change_nothing_touch_nothing() {
    let h = rules_harness();
    let session = &h.session;
    let saved = save_issue(
        session,
        0,
        "label-noop-create",
        json!({ "project_id": 1, "title": "Labelled issue", "add_labels": ["task"] }),
    );
    let before = read_issue(session, &saved.identifier);

    save_issue(
        session,
        before.version,
        "label-noop-add",
        json!({ "identifier": saved.identifier, "add_labels": ["task"] }),
    );
    let after_add = read_issue(session, &saved.identifier);
    assert_eq!(
        after_add.version, before.version,
        "re-adding a held label keeps the version"
    );
    assert_eq!(
        after_add.updated_at, before.updated_at,
        "re-adding a held label keeps updated_at"
    );

    save_issue(
        session,
        before.version,
        "label-noop-remove",
        json!({
            "identifier": saved.identifier,
            "remove_labels": ["bug"],
        }),
    );
    let after_remove = read_issue(session, &saved.identifier);
    assert_eq!(
        after_remove.version, before.version,
        "removing a label the issue lacks keeps the version"
    );
    assert_eq!(
        after_remove.updated_at, before.updated_at,
        "removing a label the issue lacks keeps updated_at"
    );
    assert_eq!(
        after_remove.labels,
        vec!["task".to_owned()],
        "the held labels stand"
    );
}

#[test]
fn removing_a_label_the_project_never_had_is_refused() {
    let h = rules_harness();
    let session = &h.session;
    let saved = save_issue(
        session,
        0,
        "label-unknown-create",
        json!({ "project_id": 1, "title": "Guarded issue" }),
    );
    let before = read_issue(session, &saved.identifier);

    let refusal = try_save(
        session,
        before.version,
        "label-unknown-remove",
        json!({
            "identifier": saved.identifier,
            "remove_labels": ["no-such-label"],
        }),
    )
    .expect_err("an unknown label name is refused");
    assert_eq!(refusal.code, ErrorCode::NotFound);
    assert_eq!(
        refusal.field.as_deref(),
        Some("remove_labels"),
        "the refusal names the list it came from"
    );
    let after = read_issue(session, &saved.identifier);
    assert_eq!(after.version, before.version);
    assert_eq!(after.updated_at, before.updated_at);
}

/// Create one ungrouped project label, the way an operator seeds a
/// spelling.
fn create_project_label(session: &OperatorSession, key: &str, name: &str) {
    session
        .call(
            "issue.create_label",
            &json!({
                "mutation": mutation(0, key),
                "project_id": 1,
                "name": name,
                "color": "#336699",
            }),
        )
        .expect("the label is created");
}

#[test]
fn adding_a_label_by_exact_spelling_beats_its_case_variants() {
    let h = rules_harness();
    let session = &h.session;
    create_project_label(session, "exact-add-lower", "web");
    create_project_label(session, "exact-add-upper", "WEB");
    let saved = save_issue(
        session,
        0,
        "exact-add-host",
        json!({ "project_id": 1, "title": "Exact spelling host" }),
    );

    save_issue(
        session,
        issue_version(session, &saved.identifier),
        "exact-add-save",
        json!({ "identifier": saved.identifier, "add_labels": ["web"] }),
    );

    assert_eq!(
        read_issue(session, &saved.identifier).labels,
        vec!["web".to_owned()],
        "the exact spelling is the label that lands, not the uppercase twin"
    );
}

#[test]
fn removing_a_held_label_by_exact_spelling_removes_it() {
    let h = rules_harness();
    let session = &h.session;
    create_project_label(session, "exact-remove-lower", "web");
    let saved = save_issue(
        session,
        0,
        "exact-remove-host",
        json!({ "project_id": 1, "title": "Exact removal host", "add_labels": ["web"] }),
    );
    create_project_label(session, "exact-remove-upper", "WEB");

    save_issue(
        session,
        issue_version(session, &saved.identifier),
        "exact-remove-save",
        json!({ "identifier": saved.identifier, "remove_labels": ["web"] }),
    );

    assert_eq!(
        read_issue(session, &saved.identifier).labels,
        Vec::<String>::new(),
        "removing the exact held spelling removes it"
    );
}

#[test]
fn re_adding_a_held_label_by_exact_spelling_touches_nothing() {
    let h = rules_harness();
    let session = &h.session;
    create_project_label(session, "exact-noop-lower", "web");
    let saved = save_issue(
        session,
        0,
        "exact-noop-host",
        json!({ "project_id": 1, "title": "Exact no-op host", "add_labels": ["web"] }),
    );
    create_project_label(session, "exact-noop-upper", "WEB");
    let before = read_issue(session, &saved.identifier);
    wait_for_clock_past(&h, &before.updated_at);

    save_issue(
        session,
        before.version,
        "exact-noop-save",
        json!({ "identifier": saved.identifier, "add_labels": ["web"] }),
    );

    let after = read_issue(session, &saved.identifier);
    assert_eq!(after.labels, before.labels, "the membership stands");
    assert_eq!(after.version, before.version, "the version stands");
    assert_eq!(
        after.updated_at, before.updated_at,
        "the update time stands"
    );
}

#[test]
fn two_case_folded_label_spellings_refuse_by_their_field() {
    let h = rules_harness();
    let session = &h.session;
    create_project_label(session, "folded-refuse-mixed", "Web");
    create_project_label(session, "folded-refuse-upper", "WEB");
    let saved = save_issue(
        session,
        0,
        "folded-refuse-host",
        json!({ "project_id": 1, "title": "Ambiguous spelling host" }),
    );
    let before = read_issue(session, &saved.identifier);
    wait_for_clock_past(&h, &before.updated_at);

    let refused = try_save(
        session,
        before.version,
        "folded-refuse-add",
        json!({
            "identifier": saved.identifier,
            "title": "A title the refusal must not land",
            "add_labels": ["web"],
        }),
    )
    .expect_err("two case-folded spellings of one name are ambiguous");
    assert_eq!(refused.code, ErrorCode::InvalidRequest);
    assert_eq!(refused.field.as_deref(), Some("add_labels"));
    let after = read_issue(session, &saved.identifier);
    assert_eq!(after.labels, before.labels, "no membership moved");
    assert_eq!(after.version, before.version, "no version moved");
    assert_eq!(after.updated_at, before.updated_at, "no update time moved");
    assert_eq!(
        after.title, before.title,
        "the rest of the refused save landed nothing"
    );

    save_issue(
        session,
        before.version,
        "folded-refuse-hold",
        json!({ "identifier": saved.identifier, "add_labels": ["Web"] }),
    );
    let holding = read_issue(session, &saved.identifier);
    let refused = try_save(
        session,
        holding.version,
        "folded-refuse-remove",
        json!({ "identifier": saved.identifier, "remove_labels": ["web"] }),
    )
    .expect_err("an ambiguous removal refuses the same way");
    assert_eq!(refused.code, ErrorCode::InvalidRequest);
    assert_eq!(refused.field.as_deref(), Some("remove_labels"));
    let after = read_issue(session, &saved.identifier);
    assert_eq!(after.labels, holding.labels, "the held spelling stands");
    assert_eq!(after.version, holding.version, "no version moved");
    assert_eq!(after.updated_at, holding.updated_at, "no update time moved");
}

#[test]
fn every_calendar_date_round_trips_through_the_due_date() {
    let h = rules_harness();
    let session = &h.session;
    let saved = save_issue(
        session,
        0,
        "due-round-trip-create",
        json!({ "project_id": 1, "title": "Dated issue" }),
    );
    let before = read_issue(session, &saved.identifier);

    save_issue(
        session,
        before.version,
        "due-round-trip-far",
        json!({
            "identifier": saved.identifier,
            "due_date": "2099-12-31",
        }),
    );
    let far = read_issue(session, &saved.identifier);
    assert_eq!(
        far.due_date.as_deref(),
        Some("2099-12-31"),
        "the farthest real calendar date is a due date, not a marker"
    );

    let cleared = save_issue(
        session,
        far.version,
        "due-round-trip-clear",
        json!({ "identifier": saved.identifier, "due_date": null }),
    );
    let _ = cleared;
    let empty = read_issue(session, &saved.identifier);
    assert_eq!(
        empty.due_date, None,
        "clearing the due date reads back as no due date"
    );

    let created = create_legacy_task(&h.core, "due-round-trip-legacy", "Legacy dated task");
    let legacy = read_issue(session, &legacy_identifier(&created));
    assert_eq!(
        legacy.due_date, None,
        "a legacy creation carries no due date"
    );
}

/// The ticket ids a global board projection shows, in board order.
fn board_tickets(h: &RulesHarness, filter: serde_json::Value) -> Vec<u64> {
    h.core
        .query("board.global", &json!({ "filter": filter }))
        .expect("the board serves")["cards"]
        .as_array()
        .expect("the cards are a list")
        .iter()
        .map(|card| card["ticket"]["id"].as_u64().expect("a ticket id"))
        .collect()
}

#[test]
fn issue_priority_changes_move_the_board_too() {
    let h = legacy_harness();
    let low = create_legacy_task_with_priority(&h.core, "board-sync-low", "Board low task", "low");
    let normal = create_legacy_task(&h.core, "board-sync-normal", "Board normal task");
    let low_id = low["id"].as_u64().expect("a ticket id");
    let normal_id = normal["id"].as_u64().expect("a ticket id");
    let low_key = legacy_identifier(&low);
    let normal_key = legacy_identifier(&normal);

    // Raising the low task to Urgent through the issue model must
    // carry the board with it: the urgent filter finds it, the low
    // filter loses it, and the board order puts it first.
    let raised = read_issue(&h.session, &low_key);
    save_issue(
        &h.session,
        raised.version,
        "board-sync-raise",
        json!({ "identifier": low_key, "priority": 1 }),
    );
    assert_eq!(
        read_issue(&h.session, &low_key).priority,
        1,
        "the issue model carries the requested priority"
    );
    assert_eq!(
        board_tickets(&h, json!({ "priorities": ["urgent"] })),
        vec![low_id],
        "the urgent filter finds the raised task"
    );
    assert_eq!(
        board_tickets(&h, json!({ "priorities": ["low"] })),
        Vec::<u64>::new(),
        "the low filter no longer holds the raised task"
    );
    assert_eq!(
        board_tickets(&h, json!({})),
        vec![low_id, normal_id],
        "the board orders the urgent task before the normal one"
    );

    // Clearing the priority entirely leaves the issue on the board:
    // No priority keeps the default normal bucket of the legacy
    // vocabulary rather than dropping or misplacing the card.
    let raised = read_issue(&h.session, &low_key);
    save_issue(
        &h.session,
        raised.version,
        "board-sync-clear",
        json!({ "identifier": low_key, "priority": 0 }),
    );
    assert_eq!(read_issue(&h.session, &low_key).priority, 0);
    let mut normal_bucket = board_tickets(&h, json!({ "priorities": ["normal"] }));
    normal_bucket.sort();
    assert_eq!(
        normal_bucket,
        vec![low_id, normal_id],
        "No priority stays on the board in the normal bucket"
    );
    assert_eq!(
        board_tickets(&h, json!({ "priorities": ["urgent"] })),
        Vec::<u64>::new(),
        "the urgent filter released the cleared task"
    );
    assert_eq!(
        board_tickets(&h, json!({})).len(),
        2,
        "no card dropped off the board"
    );

    // Lowering the normal task to Low through the issue model moves
    // it behind the No-priority task in board order.
    let cleared = read_issue(&h.session, &normal_key);
    save_issue(
        &h.session,
        cleared.version,
        "board-sync-lower",
        json!({ "identifier": normal_key, "priority": 4 }),
    );
    assert_eq!(
        board_tickets(&h, json!({})),
        vec![low_id, normal_id],
        "the low task now sorts behind the No-priority task"
    );
}

#[test]
fn direct_priority_writes_move_the_board_too() {
    let h = legacy_harness();
    let low =
        create_legacy_task_with_priority(&h.core, "board-direct-low", "Board low task", "low");
    let normal = create_legacy_task(&h.core, "board-direct-normal", "Board normal task");
    let low_id = low["id"].as_u64().expect("a ticket id");
    let normal_id = normal["id"].as_u64().expect("a ticket id");

    // The desktop editor writes the issue model's priority through
    // the store directly, without the operator surface in between:
    // the board must follow that write exactly as it follows a save.
    let store = SqliteIssueStore::new(&h.database);
    store
        .set_linear_priority(low_id as i64, 1)
        .expect("the direct raise lands");
    assert_eq!(
        board_tickets(&h, json!({ "priorities": ["urgent"] })),
        vec![low_id],
        "the urgent filter finds the directly raised task"
    );
    assert_eq!(
        board_tickets(&h, json!({ "priorities": ["low"] })),
        Vec::<u64>::new(),
        "the low filter no longer holds the directly raised task"
    );
    assert_eq!(
        board_tickets(&h, json!({})),
        vec![low_id, normal_id],
        "the board orders the directly raised task first"
    );

    // Clearing the priority through the store keeps the card on the
    // board: No priority holds the legacy normal bucket.
    store
        .set_linear_priority(low_id as i64, 0)
        .expect("the direct clear lands");
    let mut normal_bucket = board_tickets(&h, json!({ "priorities": ["normal"] }));
    normal_bucket.sort();
    assert_eq!(
        normal_bucket,
        vec![low_id, normal_id],
        "a directly cleared priority stays on the board in the normal bucket"
    );
    assert_eq!(
        board_tickets(&h, json!({ "priorities": ["urgent"] })),
        Vec::<u64>::new(),
        "the urgent filter released the directly cleared task"
    );
    assert_eq!(
        board_tickets(&h, json!({})).len(),
        2,
        "no card dropped off the board"
    );

    // Lowering the other task through the store moves it behind the
    // No-priority card in board order.
    store
        .set_linear_priority(normal_id as i64, 4)
        .expect("the direct lower lands");
    assert_eq!(
        board_tickets(&h, json!({})),
        vec![low_id, normal_id],
        "the directly lowered task sorts behind the No-priority task"
    );
}

#[test]
fn save_issue_matches_state_and_label_names_ignoring_case() {
    let h = rules_harness();
    let session = &h.session;
    let saved = save_issue(
        session,
        0,
        "case-fold-create",
        json!({ "project_id": 1, "title": "Case folded issue" }),
    );
    let identifier = saved.identifier.clone();
    let before = read_issue(session, &identifier);

    save_issue(
        session,
        before.version,
        "case-fold-state",
        json!({ "identifier": identifier, "state": "dOnE" }),
    );
    let done = read_issue(session, &identifier);
    assert_eq!(done.state, "Done", "a state name matches ignoring case");
    assert!(done.completed_at.is_some());

    save_issue(
        session,
        done.version,
        "case-fold-label",
        json!({ "identifier": identifier, "add_labels": ["BUG"] }),
    );
    assert_eq!(
        read_issue(session, &identifier).labels,
        vec!["bug".to_owned()],
        "a label name matches ignoring case"
    );

    save_issue(
        session,
        read_issue(session, &identifier).version,
        "case-fold-remove",
        json!({ "identifier": identifier, "remove_labels": ["Bug"] }),
    );
    assert_eq!(
        read_issue(session, &identifier).labels,
        Vec::<String>::new(),
        "a removed label name matches ignoring case"
    );

    let refusal = try_save(
        session,
        read_issue(session, &identifier).version,
        "case-fold-unknown-state",
        json!({ "identifier": identifier, "state": "FINISHED" }),
    )
    .expect_err("an unknown state name is still refused");
    assert_eq!(refusal.code, ErrorCode::InvalidRequest);
    assert_eq!(refusal.field.as_deref(), Some("state"));
}

#[test]
fn save_issue_accepts_a_state_type_by_board_order() {
    let h = rules_harness();
    let session = &h.session;
    let saved = save_issue(
        session,
        0,
        "state-type-create",
        json!({ "project_id": 1, "title": "State typed issue" }),
    );
    let identifier = saved.identifier.clone();
    let before = read_issue(session, &identifier);

    save_issue(
        session,
        before.version,
        "state-type-started",
        json!({ "identifier": identifier, "state": "started" }),
    );
    assert_eq!(
        read_issue(session, &identifier).state,
        "Active",
        "the started type resolves to its first state in board order"
    );

    // A board order that puts In Review first makes it the started
    // type's first state: the type follows the project's own order,
    // the way Linear uses the team's default state for the type.
    h.database
        .connection()
        .execute(
            "UPDATE issue_workflow_states SET position = 6 WHERE name = 'Active'",
            [],
        )
        .unwrap();
    h.database
        .connection()
        .execute(
            "UPDATE issue_workflow_states SET position = 5 WHERE name = 'In Review'",
            [],
        )
        .unwrap();
    let moved = read_issue(session, &identifier);
    save_issue(
        session,
        moved.version,
        "state-type-reorder",
        json!({ "identifier": identifier, "state": "Done" }),
    );
    let done = read_issue(session, &identifier);
    save_issue(
        session,
        done.version,
        "state-type-started-again",
        json!({ "identifier": identifier, "state": "STARTED" }),
    );
    assert_eq!(
        read_issue(session, &identifier).state,
        "In Review",
        "a reordered board changes the type's first state, and the type matches ignoring case"
    );
}

#[test]
fn create_label_refuses_a_blank_group() {
    let h = rules_harness();
    let refusal = h
        .session
        .call(
            "issue.create_label",
            &json!({
                "mutation": mutation(0, "blank-group"),
                "project_id": 1,
                "name": "surgical",
                "color": "#123456",
                "group": "  ",
            }),
        )
        .expect_err("a blank group is refused");
    assert_eq!(refusal.code, ErrorCode::InvalidRequest);
    assert_eq!(refusal.field.as_deref(), Some("group"));
}

#[test]
fn updated_since_accepts_a_plain_calendar_date() {
    let h = rules_harness();
    let session = &h.session;
    let saved = save_issue(
        session,
        0,
        "since-create",
        json!({ "project_id": 1, "title": "Windowed issue" }),
    );
    h.database
        .connection()
        .execute(
            "UPDATE tickets SET updated_at = '2020-01-01T00:00:00.123Z' WHERE id = ?1",
            [saved.ticket_id as i64],
        )
        .unwrap();

    let listed = |since: &str| -> usize {
        session
            .call(
                "issue.list",
                &json!({ "project_id": 1, "updated_since": since }),
            )
            .expect("the listing serves")["issues"]
            .as_array()
            .expect("an issues array")
            .len()
    };
    assert_eq!(
        listed("2020-01-01"),
        1,
        "a plain date starts at that day, UTC, so the issue counts"
    );
    assert_eq!(listed("2020-01-02"), 0, "the day after excludes the issue");
    for refused in ["2020-13-01", "2026-1-5", "2020-01-01T00:00:00.123+99:00"] {
        let error = session
            .call(
                "issue.list",
                &json!({ "project_id": 1, "updated_since": refused }),
            )
            .expect_err("a malformed window is refused");
        assert_eq!(
            (error.code, error.field.as_deref()),
            (ErrorCode::InvalidRequest, Some("updated_since")),
            "{refused} is refused with the field named"
        );
    }
}

#[test]
fn updated_since_compares_instants_across_equivalent_forms() {
    let h = rules_harness();
    let session = &h.session;
    let saved = save_issue(
        session,
        0,
        "since-instants-create",
        json!({ "project_id": 1, "title": "Instant windowed issue" }),
    );
    h.database
        .connection()
        .execute(
            "UPDATE tickets SET updated_at = '2026-11-04T22:10:30.539Z' WHERE id = ?1",
            [saved.ticket_id as i64],
        )
        .unwrap();

    let listed = |since: &str| -> usize {
        session
            .call(
                "issue.list",
                &json!({ "project_id": 1, "updated_since": since }),
            )
            .expect("the listing serves")["issues"]
            .as_array()
            .expect("an issues array")
            .len()
    };
    assert_eq!(
        listed("2026-11-04T23:10:30.539+02:00"),
        1,
        "an offset form naming an hour earlier opens the window"
    );
    assert_eq!(
        listed("2026-11-04T22:10:30Z"),
        1,
        "a whole-second form names the same instant"
    );
    assert_eq!(
        listed("2026-11-04T22:10:30.540Z"),
        0,
        "a later instant excludes the issue"
    );
    assert_eq!(
        listed("2026-11-04T00:10:30.539+00:00"),
        1,
        "an explicit zero offset compares as the same instant"
    );
    assert_eq!(
        listed("2026-11-04T22:10:30.539456Z"),
        0,
        "a sub-millisecond later instant excludes the same-millisecond stamp"
    );
    assert_eq!(
        listed("2026-11-04T22:10:30.539000Z"),
        1,
        "an exactly equal instant keeps the issue"
    );
    assert_eq!(
        listed("2026-11-04T22:10:30.538999Z"),
        1,
        "a sub-millisecond earlier instant still includes the stamp"
    );
}

#[test]
fn updated_since_accepts_iso_8601_durations_relative_to_now() {
    let h = rules_harness();
    let session = &h.session;
    let fresh = save_issue(
        session,
        0,
        "since-duration-fresh",
        json!({ "project_id": 1, "title": "Freshly updated issue" }),
    );
    save_issue(
        session,
        0,
        "since-duration-old",
        json!({ "project_id": 1, "title": "Long untouched issue" }),
    );
    h.database
        .connection()
        .execute(
            "UPDATE tickets SET updated_at = '2020-01-01T00:00:00.000Z'
             WHERE id <> ?1",
            [fresh.ticket_id as i64],
        )
        .unwrap();

    let identifiers = |since: &str| -> Vec<String> {
        session
            .call(
                "issue.list",
                &json!({ "project_id": 1, "updated_since": since }),
            )
            .expect("the listing serves")["issues"]
            .as_array()
            .expect("an issues array")
            .iter()
            .map(|record| record["identifier"].as_str().unwrap().to_owned())
            .collect()
    };
    assert_eq!(
        identifiers("-P1D"),
        vec![fresh.identifier.clone()],
        "a negative duration opens a window reaching back from now"
    );
    assert!(
        identifiers("-PT1H").contains(&fresh.identifier),
        "an hour-long window still holds the just-updated issue"
    );
    assert!(
        identifiers("P1D").is_empty(),
        "a positive duration opens a window in the future, which nothing updated reaches"
    );
    for refused in ["P1X", "-P", "PT"] {
        let error = session
            .call(
                "issue.list",
                &json!({ "project_id": 1, "updated_since": refused }),
            )
            .expect_err("a malformed duration is refused");
        assert_eq!(
            (error.code, error.field.as_deref()),
            (ErrorCode::InvalidRequest, Some("updated_since")),
            "{refused} is refused with the field named"
        );
    }
}

#[test]
fn comments_and_replies_advance_the_issue_update_time() {
    let h = rules_harness();
    let session = &h.session;
    let saved = save_issue(
        session,
        0,
        "comment-stamp-create",
        json!({ "project_id": 1, "title": "Commented issue" }),
    );
    let before = read_issue(session, &saved.identifier);
    h.database
        .connection()
        .execute(
            "UPDATE tickets SET updated_at = '2020-01-01T00:00:00.000Z' WHERE id = ?1",
            [saved.ticket_id as i64],
        )
        .unwrap();
    let target = json!({
        "kind": "ticket",
        "id": saved.ticket_id.to_string(),
    });
    let root = session
        .call(
            "comment.create",
            &json!({
                "mutation": mutation(0, "comment-stamp-root"),
                "project_id": 1,
                "target": target,
                "text": "A fresh observation",
                "author": "lin",
            }),
        )
        .expect("the comment lands");
    session
        .call(
            "comment.create",
            &json!({
                "mutation": mutation(0, "comment-stamp-reply"),
                "project_id": 1,
                "target": target,
                "text": "A reply",
                "author": "lin",
                "parent_comment_id": root["id"],
            }),
        )
        .expect("the reply lands");

    let after = read_issue(session, &saved.identifier);
    assert_ne!(
        after.updated_at, "2020-01-01T00:00:00.000Z",
        "a comment moves the issue's update time"
    );
    assert_eq!(
        after.version, before.version,
        "a comment is not an issue edit and moves no version"
    );
    let visible: Vec<String> = session
        .call(
            "issue.list",
            &json!({ "project_id": 1, "updated_since": "-P1D" }),
        )
        .expect("the listing serves")["issues"]
        .as_array()
        .expect("an issues array")
        .iter()
        .map(|record| record["identifier"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(
        visible,
        vec![saved.identifier.clone()],
        "updated_since picks the issue a comment just touched"
    );
}

#[test]
fn project_id_on_an_update_is_refused() {
    let h = rules_harness();
    let session = &h.session;
    let saved = save_issue(
        session,
        0,
        "project-refusal-create",
        json!({ "project_id": 1, "title": "Stays put" }),
    );
    let refusal = try_save(
        session,
        read_issue(session, &saved.identifier).version,
        "project-refusal-move",
        json!({ "identifier": saved.identifier, "project_id": 2, "title": "Moved" }),
    )
    .expect_err("moving an issue between projects is refused");
    assert_eq!(
        (refusal.code, refusal.field.as_deref()),
        (ErrorCode::InvalidRequest, Some("project_id"))
    );
    assert!(
        refusal
            .message
            .contains("moving issues between projects is not supported"),
        "the refusal says why: {}",
        refusal.message
    );
    let after = read_issue(session, &saved.identifier);
    assert_eq!(after.project_id, 1, "the issue stays in its project");
    assert_eq!(after.title, "Stays put", "the refused save applied nothing");
}

#[test]
fn create_label_reports_the_stored_group_flag() {
    let h = rules_harness();
    let session = &h.session;
    let first = session
        .call(
            "issue.create_label",
            &json!({
                "mutation": mutation(0, "label-flag-first"),
                "project_id": 1,
                "name": "first",
                "color": "#111111",
                "group": "Pick one",
                "group_single_select": true,
            }),
        )
        .expect("the first label lands");
    assert_eq!(first["group_single_select"], json!(true));

    let second = session
        .call(
            "issue.create_label",
            &json!({
                "mutation": mutation(0, "label-flag-second"),
                "project_id": 1,
                "name": "second",
                "color": "#222222",
                "group": "Pick one",
                "group_single_select": false,
            }),
        )
        .expect("the second label lands");
    assert_eq!(
        second["group_single_select"],
        json!(true),
        "an existing group answers with its stored flag, not the request's"
    );
}

#[test]
fn relation_lists_take_ticket_ids_and_name_their_field() {
    let h = rules_harness();
    let session = &h.session;
    let first = save_issue(
        session,
        0,
        "relations-first",
        json!({ "project_id": 1, "title": "Relating issue" }),
    );
    let second = save_issue(
        session,
        0,
        "relations-second",
        json!({ "project_id": 1, "title": "Related issue" }),
    );
    let second_id = second.ticket_id.to_string();
    let identifier = first.identifier.clone();

    save_issue(
        session,
        read_issue(session, &identifier).version,
        "relations-block",
        json!({ "identifier": identifier, "add_blocks": [second_id.clone()] }),
    );
    let blocked = read_issue(session, &identifier);
    assert_eq!(
        blocked
            .relations
            .iter()
            .filter(|relation| relation.kind == "blocks")
            .count(),
        1,
        "a bare ticket id addresses a relation target"
    );

    save_issue(
        session,
        blocked.version,
        "relations-blocked-by",
        json!({ "identifier": identifier, "add_blocked_by": [second_id.clone()] }),
    );
    save_issue(
        session,
        read_issue(session, &identifier).version,
        "relations-related",
        json!({ "identifier": identifier, "add_related": [second_id.clone()] }),
    );
    save_issue(
        session,
        read_issue(session, &identifier).version,
        "relations-remove-block",
        json!({ "identifier": identifier, "remove_blocks": [second_id.clone()] }),
    );
    save_issue(
        session,
        read_issue(session, &identifier).version,
        "relations-remove-blocked-by",
        json!({ "identifier": identifier, "remove_blocked_by": [second_id.clone()] }),
    );
    let last = save_issue(
        session,
        read_issue(session, &identifier).version,
        "relations-remove-related",
        json!({ "identifier": identifier, "remove_related": [second_id] }),
    );
    let _ = last;
    assert_eq!(
        read_issue(session, &identifier).relations,
        Vec::<kanban_dto::IssueRelationRecord>::new(),
        "every relation added by id is removed by id"
    );

    for (field, payload) in [
        ("add_blocks", json!({ "add_blocks": ["NOPE-9"] })),
        ("remove_blocks", json!({ "remove_blocks": ["NOPE-9"] })),
        ("add_related", json!({ "add_related": ["NOPE-9"] })),
        ("remove_related", json!({ "remove_related": ["NOPE-9"] })),
        ("add_blocked_by", json!({ "add_blocked_by": ["NOPE-9"] })),
        (
            "remove_blocked_by",
            json!({ "remove_blocked_by": ["NOPE-9"] }),
        ),
    ] {
        let mut fields = json!({ "identifier": identifier });
        if let Some(object) = fields.as_object_mut() {
            for (key, value) in payload.as_object().expect("an object") {
                object.insert(key.clone(), value.clone());
            }
        }
        let refusal = try_save(
            session,
            read_issue(session, &identifier).version,
            &format!("relations-refusal-{field}"),
            fields,
        )
        .expect_err("an unknown relation target is refused");
        assert_eq!(
            (refusal.code, refusal.field.as_deref()),
            (ErrorCode::NotFound, Some(field)),
            "the refusal names the list it came from"
        );
    }
}
#[test]
fn an_explicit_legacy_priority_lands_after_no_priority() {
    let h = legacy_harness();
    let created = create_legacy_task_with_priority(
        &h.core,
        "explicit-priority-create",
        "Explicitly prioritised task",
        "low",
    );
    let identifier = legacy_identifier(&created);
    let ticket_id = created["id"].as_u64().expect("a ticket id");

    // No priority keeps the legacy normal bucket on the board while
    // the issue itself carries 0.
    let cleared = read_issue(&h.session, &identifier);
    save_issue(
        &h.session,
        cleared.version,
        "explicit-priority-clear",
        json!({ "identifier": identifier, "priority": 0 }),
    );
    assert_eq!(read_issue(&h.session, &identifier).priority, 0);
    assert!(board_tickets(&h, json!({ "priorities": ["normal"] })).contains(&ticket_id));

    // An unrelated legacy edit preserves the operator's No priority.
    let edited = read_issue(&h.session, &identifier);
    h.core
        .command(
            "ticket.edit",
            &json!({
                "mutation": mutation(edited.version, "explicit-priority-edit"),
                "ticket_id": ticket_id,
                "title": "Edited beside No priority",
            }),
        )
        .expect("the legacy edit lands");
    assert_eq!(
        read_issue(&h.session, &identifier).priority,
        0,
        "an unrelated legacy edit preserves No priority"
    );

    // An explicit legacy priority lands on the issue model with the
    // migration's mapping — including normal onto the normal bucket
    // the row already occupies after No priority.
    for (step, (legacy, mapped, bucket)) in [
        ("normal", 3u8, "normal"),
        ("urgent", 1, "urgent"),
        ("high", 2, "high"),
        ("low", 4, "low"),
        ("normal", 3, "normal"),
    ]
    .into_iter()
    .enumerate()
    {
        let before = read_issue(&h.session, &identifier);
        h.core
            .command(
                "ticket.prioritise",
                &json!({
                    "mutation": mutation(before.version, format!("explicit-priority-{step}-{legacy}")),
                    "ticket_id": ticket_id,
                    "priority": legacy,
                }),
            )
            .expect("the legacy prioritise lands");
        let after = read_issue(&h.session, &identifier);
        assert_eq!(
            after.priority, mapped,
            "an explicit legacy {legacy} moves the issue priority"
        );
        assert_eq!(after.version, before.version + 1,);
        assert_eq!(
            after.state_history.len(),
            before.state_history.len(),
            "a priority change rewrites no history"
        );
        assert!(
            board_tickets(&h, json!({ "priorities": [bucket] })).contains(&ticket_id),
            "the {bucket} filter holds the ticket"
        );
    }
}

#[test]
fn a_scheduled_occurrence_opens_its_ready_state_row() {
    use kanban_app::recurrence::RecurrenceStore;
    use kanban_domain::recurrence::RecurrenceAdvance;
    use kanban_storage::recurrence::SqliteRecurrenceStore;

    let h = legacy_harness();
    let created = create_legacy_task_with_priority(
        &h.core,
        "recurrence-seed-create",
        "Recurring high task",
        "high",
    );
    let identifier = legacy_identifier(&created);
    let ticket_id = created["id"].as_u64().expect("a ticket id");

    h.core
        .command(
            "ticket.schedule",
            &json!({
                "mutation": mutation(read_issue(&h.session, &identifier).version, "recurrence-seed-schedule"),
                "ticket_id": ticket_id,
                "cron": "*/15 * * * *",
                "timezone": "UTC",
                "after": "2026-09-08T09:00:00Z",
                "profile": "standard",
            }),
        )
        .expect("the legacy schedule lands");
    let schedule_id: i64 = h
        .database
        .connection()
        .query_row(
            "SELECT id FROM schedules WHERE ticket_id = ?1",
            [ticket_id as i64],
            |row| row.get(0),
        )
        .expect("the schedule row lands");

    let committed = SqliteRecurrenceStore::new(&h.database)
        .advance(
            kanban_domain::ScheduleId::new(schedule_id as u64),
            "2026-11-04T21:00:00.000Z",
            &|_| {
                Ok(Some(RecurrenceAdvance {
                    occurrence_window: Some("2026-11-04T20:45:00.000Z".to_owned()),
                    skipped: None,
                    next_activation: "2026-11-04T21:15:00.000Z".to_owned(),
                }))
            },
        )
        .expect("the due recurrence advances")
        .expect("the decision mints an occurrence");
    let occurrence = committed.ticket.expect("the occurrence is committed");
    let occurrence_key = format!("CORE-{}", occurrence.number().value());

    // The occurrence is a full issue the moment it is minted: the
    // stored workflow projection holds Ready with its open history
    // row, and the template's high priority maps to 2 the way every
    // legacy creation does.
    let record = read_issue(&h.session, &occurrence_key);
    assert_eq!(record.state, "Ready", "the stored projection holds Ready");
    assert_eq!(record.state_type, "unstarted");
    let names = history_names(&record);
    assert_eq!(
        names,
        vec!["Ready"],
        "the occurrence opens exactly its Ready row, saw {names:?}"
    );
    assert!(record.state_history[0].ended_at.is_none());
    assert_eq!(record.priority, 2, "the template's high maps to 2");
    assert!(record.started_at.is_none());
    assert!(record.completed_at.is_none());
    assert!(record.canceled_at.is_none());

    // A second pass before the next activation mints nothing, so the
    // occurrence keeps exactly one open history row.
    let idle = SqliteRecurrenceStore::new(&h.database)
        .advance(
            kanban_domain::ScheduleId::new(schedule_id as u64),
            "2026-11-04T21:00:00.000Z",
            &|_| {
                Ok(Some(RecurrenceAdvance {
                    occurrence_window: Some("2026-11-04T21:00:00.000Z".to_owned()),
                    skipped: None,
                    next_activation: "2026-11-04T21:30:00.000Z".to_owned(),
                }))
            },
        )
        .expect("the idle pass refuses nothing");
    assert!(idle.is_none(), "a not-yet-due schedule mints nothing");
    let unchanged = read_issue(&h.session, &occurrence_key);
    assert_eq!(
        unchanged.state_history.len(),
        1,
        "no retry duplicates the occurrence's history"
    );
}
