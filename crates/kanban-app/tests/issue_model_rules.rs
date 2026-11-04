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
        database: h.database,
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
