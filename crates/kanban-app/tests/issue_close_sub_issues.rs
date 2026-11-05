//! The per-project `close_sub_issues_with_parent` setting, Linear's
//! "auto-close sub-issues": when a parent closes, its open sub-issues
//! close with it. A completed parent completes them; a canceled,
//! superseded, or duplicate parent cancels them. Off by default, it
//! moves each child as any state move does and never loops with the
//! parent auto-close rule.

use std::sync::{Arc, Mutex};

use kanban_app::{
    CommentIssueTargets, CommentStore, Core, EventSink, IssueStore, IssueStoreCommentTargets,
    OperatorSession, ProjectStore, TimelineEnvelope,
};
use kanban_domain::ProjectRegistration;
use kanban_dto::{
    ApiError, IssueGetQuery, IssueProjectRecord, IssueProjectSettingsResponse,
    IssueProjectsResponse, IssueRecord, IssueSaveResponse, TimelineEntityKind, TimelineEntityRef,
    TimelineEventKind,
};
use kanban_storage::{
    Database, SqliteCommentStore, SqliteDependencyStore, SqliteEvidenceStore, SqliteIssueStore,
    SqlitePlanStore, SqliteProjectStore, SqliteScheduleStore, SqliteSpecStore, SqliteTicketStore,
};
use serde_json::{Value, json};

mod common;

use common::{core_over_with_events, harness, mutation, seed_project_profile};

const CORE: u64 = 1;
const EDGE: u64 = 2;

/// The operator issue tools beside the legacy Ticket lifecycle, so a
/// parent can close either way.
fn register_stack(core: &mut Core, database: &Database, attachments: &std::path::Path) {
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
    core.register_lifecycle(
        tickets.clone(),
        Arc::new(SqliteDependencyStore::new(database)),
        projects.clone(),
        Arc::new(SqliteScheduleStore::new(database)),
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

struct Harness {
    _dir: tempfile::TempDir,
    core: Arc<Core>,
    session: OperatorSession,
    database_path: std::path::PathBuf,
}

fn harness_with_stack() -> Harness {
    let mut h = harness();
    let attachments = h._dir.path().join("attachments");
    register_stack(&mut h.core, &h.database, &attachments);
    let core = Arc::new(h.core);
    Harness {
        session: OperatorSession::new(core.clone()),
        core,
        database_path: h.database_path,
        _dir: h._dir,
    }
}

fn payload(version: u64, key: &str, fields: Value) -> Value {
    let mut object = serde_json::Map::new();
    object.insert("mutation".into(), mutation(version, key));
    if let Some(extra) = fields.as_object() {
        object.extend(extra.clone());
    }
    Value::Object(object)
}

fn save(session: &OperatorSession, key: &str, fields: Value) -> IssueSaveResponse {
    serde_json::from_value(
        session
            .call("issue.save", &payload(0, key, fields))
            .expect("issue.save"),
    )
    .expect("save shape")
}

fn child_of(
    session: &OperatorSession,
    parent: &IssueSaveResponse,
    key: &str,
    state: &str,
) -> IssueSaveResponse {
    save(
        session,
        key,
        json!({
            "project_id": CORE,
            "title": format!("Sub-issue {key}"),
            "state": state,
            "parent_id": parent.identifier,
        }),
    )
}

fn move_to(session: &OperatorSession, identifier: &str, key: &str, state: &str) {
    session
        .call(
            "issue.save",
            &payload(
                read(session, identifier).version,
                key,
                json!({ "identifier": identifier, "state": state }),
            ),
        )
        .expect("the state move lands");
}

fn read(session: &OperatorSession, identifier: &str) -> IssueRecord {
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

fn settings(
    session: &OperatorSession,
    key: &str,
    fields: Value,
) -> Result<IssueProjectSettingsResponse, ApiError> {
    let mut request = payload(0, key, fields);
    request["project_id"] = json!(CORE);
    session
        .call("issue.update_project_settings", &request)
        .map(|value| serde_json::from_value(value).expect("settings shape"))
}

fn close_with_parent(session: &OperatorSession) {
    settings(
        session,
        "close-with-parent-on",
        json!({ "close_sub_issues_with_parent": true }),
    )
    .expect("the setting is enabled");
}

fn history_names(record: &IssueRecord) -> Vec<&str> {
    record
        .state_history
        .iter()
        .map(|row| row.state.as_str())
        .collect()
}

fn count_history_rows(h: &Harness, ticket_id: u64, state: &str) -> i64 {
    let conn = rusqlite::Connection::open(&h.database_path).unwrap();
    conn.query_row(
        "SELECT COUNT(*) FROM issue_state_history AS h
         JOIN issue_workflow_states AS s ON s.id = h.workflow_state_id
         WHERE h.ticket_id = ?1 AND s.name = ?2",
        rusqlite::params![ticket_id as i64, state],
        |row| row.get(0),
    )
    .unwrap()
}

/// The timeline events that record a sub-issue closing with its
/// parent: the state it moved to and the parent that moved it.
fn cascade_events(h: &Harness, ticket_id: u64) -> Vec<(String, i64)> {
    let conn = rusqlite::Connection::open(&h.database_path).unwrap();
    let mut statement = conn
        .prepare(
            "SELECT json_extract(detail, '$.state'), json_extract(detail, '$.parent_ticket_id')
             FROM timeline_events
             WHERE entity_kind = 'ticket' AND entity_id = ?1
               AND json_extract(detail, '$.action') = 'close_sub_issues_with_parent'
             ORDER BY id",
        )
        .unwrap();
    statement
        .query_map([ticket_id.to_string()], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}

#[test]
fn the_setting_is_off_until_enabled_and_leaves_the_other_alone() {
    let h = harness_with_stack();
    let s = &h.session;

    let initial = settings(s, "read-only", json!({})).expect("an empty update reads");
    assert!(!initial.close_sub_issues_with_parent, "off by default");
    assert!(!initial.auto_close_sub_issues);

    let enabled = settings(s, "enable", json!({ "close_sub_issues_with_parent": true })).unwrap();
    assert!(enabled.close_sub_issues_with_parent);
    assert!(
        !enabled.auto_close_sub_issues,
        "setting one leaves the parent auto-close setting as it was"
    );

    let other = settings(s, "other", json!({ "auto_close_sub_issues": true })).unwrap();
    assert!(other.auto_close_sub_issues);
    assert!(
        other.close_sub_issues_with_parent,
        "setting the other leaves this one as it was"
    );

    let disabled = settings(
        s,
        "disable",
        json!({ "close_sub_issues_with_parent": false }),
    )
    .unwrap();
    assert!(!disabled.close_sub_issues_with_parent);
}

#[test]
fn closing_a_parent_leaves_its_sub_issues_open_while_the_setting_is_off() {
    let h = harness_with_stack();
    let s = &h.session;
    let parent = save(
        s,
        "parent",
        json!({ "project_id": CORE, "title": "Parent", "state": "Active" }),
    );
    let child = child_of(s, &parent, "child", "Active");

    move_to(s, &parent.identifier, "parent-done", "Done");

    let open = read(s, &child.identifier);
    assert_eq!(open.state, "Active");
    assert_eq!(open.completed_at, None);
}

#[test]
fn completing_a_parent_completes_its_open_sub_issues() {
    let h = harness_with_stack();
    let s = &h.session;
    close_with_parent(s);
    let parent = save(
        s,
        "parent",
        json!({ "project_id": CORE, "title": "Parent", "state": "Active" }),
    );
    let drafted = child_of(s, &parent, "drafted", "Draft");
    let ready = child_of(s, &parent, "ready", "Ready");
    let reviewing = child_of(s, &parent, "reviewing", "In Review");
    let cancelled = child_of(s, &parent, "cancelled", "Cancelled");
    let cancelled_before = read(s, &cancelled.identifier);

    move_to(s, &parent.identifier, "parent-done", "Done");

    for child in [&drafted, &ready, &reviewing] {
        let closed = read(s, &child.identifier);
        assert_eq!(
            closed.state, "Done",
            "{} completes with its parent",
            child.identifier
        );
        assert_eq!(closed.state_type, "completed");
        assert!(
            closed.completed_at.is_some(),
            "{} is stamped completed",
            child.identifier
        );
        assert_eq!(closed.canceled_at, None);
        let last = closed.state_history.last().expect("a history row");
        assert_eq!(last.state, "Done");
        assert_eq!(last.ended_at, None, "the Done row is the open one");
        assert!(
            closed.state_history[..closed.state_history.len() - 1]
                .iter()
                .all(|row| row.ended_at.is_some()),
            "the state {} left is closed off",
            child.identifier
        );
    }
    assert_eq!(
        history_names(&read(s, &reviewing.identifier)),
        vec!["In Review", "Done"]
    );
    for child in [&drafted, &ready, &reviewing] {
        assert_eq!(
            cascade_events(&h, child.ticket_id),
            vec![("Done".to_owned(), parent.ticket_id as i64)],
            "{} records why it moved",
            child.identifier
        );
    }
    assert!(cascade_events(&h, cancelled.ticket_id).is_empty());
    let untouched = read(s, &cancelled.identifier);
    assert_eq!(
        untouched.state, "Cancelled",
        "a closed sub-issue keeps its state"
    );
    assert_eq!(untouched.version, cancelled_before.version);
    assert_eq!(untouched.state_history, cancelled_before.state_history);
}

#[test]
fn cancelling_a_parent_cancels_its_open_sub_issues() {
    let h = harness_with_stack();
    let s = &h.session;
    close_with_parent(s);
    let parent = save(
        s,
        "parent",
        json!({ "project_id": CORE, "title": "Parent", "state": "Active" }),
    );
    let open = child_of(s, &parent, "open", "Active");
    let done = child_of(s, &parent, "done", "Done");

    move_to(s, &parent.identifier, "parent-cancelled", "Cancelled");

    let cancelled = read(s, &open.identifier);
    assert_eq!(cancelled.state, "Cancelled");
    assert!(cancelled.canceled_at.is_some());
    assert_eq!(cancelled.completed_at, None);
    assert_eq!(history_names(&cancelled), vec!["Active", "Cancelled"]);
    let finished = read(s, &done.identifier);
    assert_eq!(finished.state, "Done", "finished work stays finished");
    assert!(finished.completed_at.is_some());
}

#[test]
fn a_superseded_or_duplicate_parent_cancels_its_open_sub_issues() {
    let h = harness_with_stack();
    let s = &h.session;
    close_with_parent(s);
    let superseded = save(
        s,
        "superseded",
        json!({ "project_id": CORE, "title": "Superseded parent" }),
    );
    let under_superseded = child_of(s, &superseded, "under-superseded", "Ready");
    let duplicate = save(
        s,
        "duplicate",
        json!({ "project_id": CORE, "title": "Duplicate parent" }),
    );
    let under_duplicate = child_of(s, &duplicate, "under-duplicate", "Ready");
    let canonical = save(
        s,
        "canonical",
        json!({ "project_id": CORE, "title": "Canonical issue" }),
    );

    move_to(s, &superseded.identifier, "supersede", "Superseded");
    s.call(
        "issue.save",
        &payload(
            read(s, &duplicate.identifier).version,
            "mark-duplicate",
            json!({ "identifier": duplicate.identifier, "duplicate_of": canonical.identifier }),
        ),
    )
    .expect("the duplicate lands");

    for child in [&under_superseded, &under_duplicate] {
        let closed = read(s, &child.identifier);
        assert_eq!(
            closed.state, "Cancelled",
            "{} is cancelled",
            child.identifier
        );
        assert!(closed.canceled_at.is_some());
    }
    assert_eq!(read(s, &duplicate.identifier).state, "Duplicate");
}

#[test]
fn closing_cascades_through_every_level() {
    let h = harness_with_stack();
    let s = &h.session;
    close_with_parent(s);
    let top = save(
        s,
        "top",
        json!({ "project_id": CORE, "title": "Top", "state": "Active" }),
    );
    let middle = child_of(s, &top, "middle", "Active");
    let bottom = child_of(s, &middle, "bottom", "Draft");

    move_to(s, &top.identifier, "top-done", "Done");

    assert_eq!(read(s, &middle.identifier).state, "Done");
    assert_eq!(read(s, &bottom.identifier).state, "Done");
}

#[test]
fn both_settings_together_close_each_issue_once() {
    let h = harness_with_stack();
    let s = &h.session;
    close_with_parent(s);
    settings(
        s,
        "parent-auto-close",
        json!({ "auto_close_sub_issues": true }),
    )
    .unwrap();
    let top = save(
        s,
        "top",
        json!({ "project_id": CORE, "title": "Top", "state": "Active" }),
    );
    let parent = child_of(s, &top, "parent", "Active");
    let first = child_of(s, &parent, "first", "Active");
    let second = child_of(s, &parent, "second", "Active");

    move_to(s, &parent.identifier, "parent-done", "Done");

    for issue in [&top, &parent, &first, &second] {
        let record = read(s, &issue.identifier);
        assert_eq!(record.state, "Done", "{} is closed", issue.identifier);
        assert_eq!(
            count_history_rows(&h, issue.ticket_id, "Done"),
            1,
            "{} enters Done exactly once",
            issue.identifier
        );
    }
}

#[test]
fn reopening_a_parent_leaves_its_closed_sub_issues_closed() {
    let h = harness_with_stack();
    let s = &h.session;
    close_with_parent(s);
    let parent = save(
        s,
        "parent",
        json!({ "project_id": CORE, "title": "Parent", "state": "Active" }),
    );
    let child = child_of(s, &parent, "child", "Active");
    move_to(s, &parent.identifier, "parent-done", "Done");

    move_to(s, &parent.identifier, "parent-reopened", "Active");

    assert_eq!(read(s, &parent.identifier).state, "Active");
    assert_eq!(read(s, &child.identifier).state, "Done");
}

#[test]
fn a_legacy_parent_cancel_cancels_its_open_sub_issues() {
    let h = harness_with_stack();
    let s = &h.session;
    close_with_parent(s);
    let parent = save(
        s,
        "parent",
        json!({ "project_id": CORE, "title": "Parent", "state": "Ready" }),
    );
    let child = child_of(s, &parent, "child", "Ready");

    h.core
        .command(
            "ticket.cancel",
            &json!({
                "mutation": mutation(read(s, &parent.identifier).version, "legacy-cancel"),
                "ticket_id": parent.ticket_id,
            }),
        )
        .expect("the legacy cancel lands");

    assert_eq!(read(s, &parent.identifier).state, "Cancelled");
    let cancelled = read(s, &child.identifier);
    assert_eq!(
        cancelled.state, "Cancelled",
        "the sub-issue closes in the same write as the legacy move"
    );
    assert!(cancelled.canceled_at.is_some());
}
#[test]
fn list_projects_reads_both_sub_issue_settings() {
    let h = harness_with_stack();
    let s = &h.session;
    let projects = |s: &OperatorSession| -> Vec<IssueProjectRecord> {
        serde_json::from_value::<IssueProjectsResponse>(
            s.call("issue.list_projects", &json!({}))
                .expect("issue.list_projects"),
        )
        .expect("projects shape")
        .projects
    };

    let before = projects(s);
    assert_eq!(
        before,
        vec![IssueProjectRecord {
            id: CORE,
            code: "CORE".to_owned(),
            name: "Control plane".to_owned(),
            auto_close_sub_issues: false,
            close_sub_issues_with_parent: false,
        }],
        "both settings read off until a project enables them"
    );

    close_with_parent(s);
    let after = projects(s);
    assert!(!after[0].auto_close_sub_issues);
    assert!(after[0].close_sub_issues_with_parent);

    settings(
        s,
        "parent-auto-close",
        json!({ "auto_close_sub_issues": true }),
    )
    .unwrap();
    let both = projects(s);
    assert!(both[0].auto_close_sub_issues);
    assert!(both[0].close_sub_issues_with_parent);
}
fn plant_executing_run(database_path: &std::path::Path, ticket_id: i64) {
    let conn = rusqlite::Connection::open(database_path).unwrap();
    conn.execute(
        "INSERT INTO dispatch_requests (
             project_id, ticket_id, status, priority, ready, harness, model, usage_pool,
             created_at, version
         ) VALUES (1, ?1, 'claimed', 'normal', 1, 'sample', 'sample', 'default', 0, 1)",
        [ticket_id],
    )
    .expect("the dispatch request lands");
    let request_id = conn.last_insert_rowid();
    conn.execute(
        "INSERT INTO runs (
             project_id, ticket_id, dispatch_request_id, status,
             requested_name, requested_harness, requested_model, requested_effort,
             requested_usage_pool, effective_name, effective_harness, effective_model,
             effective_effort, effective_usage_pool, fallback, created_at, version
         ) VALUES (
             1, ?1, ?2, 'executing',
             'standard', 'sample', 'sample', 'high',
             'default', 'standard', 'sample', 'sample',
             'high', 'default', 0, 0, 1
         )",
        [ticket_id, request_id],
    )
    .expect("the executing run lands");
}

#[test]
fn an_executing_child_stays_open_when_its_parent_closes() {
    let h = harness_with_stack();
    let s = &h.session;
    close_with_parent(s);
    let parent = save(
        s,
        "parent",
        json!({ "project_id": CORE, "title": "Parent" }),
    );
    let busy = child_of(s, &parent, "busy", "Active");
    let quiet = child_of(s, &parent, "quiet", "Active");
    let grandchild = child_of(s, &busy, "grandchild", "Active");
    plant_executing_run(&h.database_path, busy.ticket_id as i64);
    let before = read(s, &busy.identifier);

    move_to(s, &parent.identifier, "parent-done", "Done");

    let after = read(s, &busy.identifier);
    assert_eq!(
        after.state, "Active",
        "a child with its own executing run stays open"
    );
    assert_eq!(
        after.version, before.version,
        "a refused close leaves the child unmoved"
    );
    assert!(
        cascade_events(&h, busy.ticket_id).is_empty(),
        "a refused close records no close, the same as any refused move"
    );
    assert_eq!(read(s, &quiet.identifier).state, "Done");
    assert_eq!(
        read(s, &grandchild.identifier).state,
        "Active",
        "descent stops at the child that stayed open"
    );
}

/// Live announcements captured from the core the test built.
#[derive(Default)]
struct RecordingEvents {
    events: Mutex<Vec<(String, Value)>>,
}

impl RecordingEvents {
    fn clear(&self) {
        self.events.lock().expect("the event log is sound").clear();
    }

    fn named(&self, kind: &str) -> Vec<Value> {
        self.events
            .lock()
            .expect("the event log is sound")
            .iter()
            .filter(|(event_kind, _)| event_kind == kind)
            .map(|(_, payload)| payload.clone())
            .collect()
    }

    fn state_changes(&self) -> Vec<(u64, Value)> {
        self.events
            .lock()
            .expect("the event log is sound")
            .iter()
            .filter(|(kind, _)| kind == "ticket.state.changed")
            .map(|(_, payload)| {
                (
                    payload["id"]
                        .as_u64()
                        .expect("a state change names an issue"),
                    payload["state"].clone(),
                )
            })
            .collect()
    }
}

impl EventSink for RecordingEvents {
    fn emit(&self, event_type: &str, payload: Value) {
        self.events
            .lock()
            .expect("the event log is sound")
            .push((event_type.to_owned(), payload));
    }
}

fn seed_edge_project(database: &Database) {
    let projects = SqliteProjectStore::new(database);
    let registration = ProjectRegistration::new(
        "EDGE",
        "Edge",
        "/repositories/edge",
        "/workspaces/edge.seed",
        "main",
        "edge.seed",
        Some("edge-main"),
        None,
    )
    .expect("the edge registration validates");
    projects
        .create(&registration, &|id| {
            TimelineEnvelope::project(
                id.value(),
                TimelineEventKind::Transition,
                Some(TimelineEntityRef {
                    kind: TimelineEntityKind::Project,
                    id: id.value().to_string(),
                }),
                json!({ "action": "registered" }),
            )
        })
        .expect("the edge project lands");
}

fn recording_stack() -> (Harness, Arc<RecordingEvents>) {
    let dir = tempfile::TempDir::new().expect("a scratch directory is available");
    let database_path = dir.path().join("kanban.sqlite");
    let mut database = Database::open(&database_path).expect("a scratch database opens");
    database
        .migrate(&kanban_storage::AllowAllMigrations)
        .expect("the migrations apply");
    seed_project_profile(&database);
    seed_edge_project(&database);
    let events = Arc::new(RecordingEvents::default());
    let (mut core, _wake) = core_over_with_events(&database, events.clone());
    let attachments = dir.path().join("attachments");
    register_stack(&mut core, &database, &attachments);
    let core = Arc::new(core);
    (
        Harness {
            session: OperatorSession::new(core.clone()),
            core,
            database_path,
            _dir: dir,
        },
        events,
    )
}

fn child_in_project(
    session: &OperatorSession,
    parent: &IssueSaveResponse,
    key: &str,
    project_id: u64,
    state: &str,
) -> IssueSaveResponse {
    save(
        session,
        key,
        json!({
            "project_id": project_id,
            "title": format!("Sub-issue {key}"),
            "state": state,
            "parent_id": parent.identifier,
        }),
    )
}

struct CascadeTree {
    parent: IssueSaveResponse,
    busy: IssueSaveResponse,
    busy_child: IssueSaveResponse,
    quiet: IssueSaveResponse,
    quiet_child: IssueSaveResponse,
    edge: IssueSaveResponse,
    edge_child: IssueSaveResponse,
    already: IssueSaveResponse,
}

fn cascade_tree(session: &OperatorSession, prefix: &str) -> CascadeTree {
    close_with_parent(session);
    let parent = save(
        session,
        &format!("{prefix}-parent"),
        json!({ "project_id": CORE, "title": "Parent", "state": "Ready" }),
    );
    let busy = child_of(session, &parent, &format!("{prefix}-busy"), "Ready");
    let busy_child = child_of(session, &busy, &format!("{prefix}-busy-child"), "Ready");
    let quiet = child_of(session, &parent, &format!("{prefix}-quiet"), "Ready");
    let quiet_child = child_of(session, &quiet, &format!("{prefix}-quiet-child"), "Ready");
    let edge = child_in_project(session, &parent, &format!("{prefix}-edge"), EDGE, "Ready");
    let edge_child = child_in_project(
        session,
        &edge,
        &format!("{prefix}-edge-child"),
        EDGE,
        "Ready",
    );
    let already = child_of(session, &parent, &format!("{prefix}-already"), "Done");
    CascadeTree {
        parent,
        busy,
        busy_child,
        quiet,
        quiet_child,
        edge,
        edge_child,
        already,
    }
}

fn assert_cancelled_ids_announced(events: &RecordingEvents, closed: &[u64], open: &[u64]) {
    let changes = events.state_changes();
    let announced: Vec<u64> = changes.iter().map(|(id, _)| *id).collect();
    for id in closed {
        let states: Vec<_> = changes
            .iter()
            .filter(|(changed, _)| changed == id)
            .map(|(_, state)| state.clone())
            .collect();
        assert_eq!(
            states,
            vec![json!("cancelled")],
            "issue {id} is announced once as cancelled, saw {changes:?}"
        );
    }
    for id in open {
        assert!(
            !announced.contains(id),
            "issue {id} must not be announced, saw {changes:?}"
        );
    }
    assert_eq!(
        announced.len(),
        closed.len(),
        "only the closed issues are announced, saw {changes:?}"
    );
}

fn closed_by_cancel(tree: &CascadeTree) -> Vec<u64> {
    vec![
        tree.parent.ticket_id,
        tree.quiet.ticket_id,
        tree.quiet_child.ticket_id,
        tree.edge.ticket_id,
    ]
}

fn left_open_by_cancel(tree: &CascadeTree) -> Vec<u64> {
    vec![
        tree.busy.ticket_id,
        tree.busy_child.ticket_id,
        tree.edge_child.ticket_id,
        tree.already.ticket_id,
    ]
}

fn assert_cancel_tree(session: &OperatorSession, tree: &CascadeTree) {
    assert_eq!(read(session, &tree.parent.identifier).state, "Cancelled");
    assert_eq!(read(session, &tree.quiet.identifier).state, "Cancelled");
    assert_eq!(
        read(session, &tree.quiet_child.identifier).state,
        "Cancelled"
    );
    assert_eq!(read(session, &tree.edge.identifier).state, "Cancelled");
    assert_eq!(read(session, &tree.busy.identifier).state, "Ready");
    assert_eq!(read(session, &tree.busy_child.identifier).state, "Ready");
    assert_eq!(read(session, &tree.edge_child.identifier).state, "Ready");
    assert_eq!(read(session, &tree.already.identifier).state, "Done");
}

#[test]
fn a_legacy_cancel_announces_every_child_the_cascade_closes() {
    let (h, events) = recording_stack();
    let session = &h.session;
    let tree = cascade_tree(session, "legacy");
    plant_executing_run(&h.database_path, tree.busy.ticket_id as i64);
    let busy_before = read(session, &tree.busy.identifier);
    let already_before = read(session, &tree.already.identifier);
    let edge_child_before = read(session, &tree.edge_child.identifier);

    events.clear();
    h.core
        .command(
            "ticket.cancel",
            &json!({
                "mutation": mutation(
                    read(session, &tree.parent.identifier).version,
                    "legacy-cancel-announce"
                ),
                "ticket_id": tree.parent.ticket_id,
            }),
        )
        .expect("the legacy cancel lands");

    assert_cancel_tree(session, &tree);
    assert_eq!(read(session, &tree.busy.identifier), busy_before);
    assert_eq!(read(session, &tree.already.identifier), already_before);
    assert_eq!(
        read(session, &tree.edge_child.identifier),
        edge_child_before
    );
    assert_cancelled_ids_announced(
        &events,
        &closed_by_cancel(&tree),
        &left_open_by_cancel(&tree),
    );
}

#[test]
fn an_operator_close_announces_every_child_the_cascade_closes() {
    let (h, events) = recording_stack();
    let session = &h.session;
    let tree = cascade_tree(session, "operator");
    plant_executing_run(&h.database_path, tree.busy.ticket_id as i64);
    let busy_before = read(session, &tree.busy.identifier);

    events.clear();
    move_to(
        session,
        &tree.parent.identifier,
        "operator-cancel",
        "Cancelled",
    );

    assert_cancel_tree(session, &tree);
    assert_eq!(read(session, &tree.busy.identifier), busy_before);
    assert_cancelled_ids_announced(
        &events,
        &closed_by_cancel(&tree),
        &left_open_by_cancel(&tree),
    );
}

#[test]
fn a_refused_legacy_cancel_publishes_no_state_change() {
    let (h, events) = recording_stack();
    let session = &h.session;
    close_with_parent(session);
    let parent = save(
        session,
        "refused-parent",
        json!({ "project_id": CORE, "title": "Parent", "state": "Ready" }),
    );
    let child = child_of(session, &parent, "refused-child", "Ready");
    let parent_before = read(session, &parent.identifier);
    let child_before = read(session, &child.identifier);

    events.clear();
    let error = h
        .core
        .command(
            "ticket.cancel",
            &json!({
                "mutation": mutation(0, "legacy-cancel-stale"),
                "ticket_id": parent.ticket_id,
            }),
        )
        .expect_err("a stale cancel is refused");

    assert_eq!(error.code, kanban_dto::ErrorCode::StaleVersion);
    assert_eq!(read(session, &parent.identifier), parent_before);
    assert_eq!(read(session, &child.identifier), child_before);
    assert!(
        events.state_changes().is_empty(),
        "a refused cancel publishes no state change, saw {:?}",
        events.state_changes()
    );

    h.core
        .command(
            "ticket.cancel",
            &json!({
                "mutation": mutation(parent_before.version, "legacy-cancel-after-refusal"),
                "ticket_id": parent.ticket_id,
            }),
        )
        .expect("the later cancel lands");
    assert_cancelled_ids_announced(&events, &[parent.ticket_id, child.ticket_id], &[]);
}

fn reassignment_of(ticket_id: u64, version: u64, key: &str) -> Value {
    json!({
        "mutation": mutation(version, key),
        "ticket_id": ticket_id,
        "kind": "task",
        "priority": "normal",
        "title": "Replacement task",
        "subtype": "operational",
        "mode": "human",
        "completion": ["Checked"],
    })
}

fn closed_by_reassignment(tree: &CascadeTree) -> Vec<u64> {
    vec![
        tree.quiet.ticket_id,
        tree.quiet_child.ticket_id,
        tree.edge.ticket_id,
    ]
}

fn assert_reassignment_announces_closed_children(
    events: &RecordingEvents,
    parent_id: u64,
    closed: &[u64],
    open: &[u64],
) {
    let changes = events.state_changes();
    for id in closed {
        let states: Vec<_> = changes
            .iter()
            .filter(|(changed, _)| changed == id)
            .map(|(_, state)| state.clone())
            .collect();
        assert_eq!(
            states,
            vec![json!("cancelled")],
            "issue {id} is announced once as cancelled, saw {changes:?}"
        );
    }
    for id in open {
        assert!(
            !changes.iter().any(|(changed, _)| changed == id),
            "issue {id} must not be announced, saw {changes:?}"
        );
    }
    let parent_states: Vec<_> = changes
        .iter()
        .filter(|(id, _)| *id == parent_id)
        .map(|(_, state)| state.clone())
        .collect();
    assert_eq!(
        parent_states,
        vec![json!("superseded")],
        "the original is announced once as superseded, saw {changes:?}"
    );
    let extras: Vec<_> = changes
        .iter()
        .filter(|(id, _)| *id != parent_id && !closed.contains(id))
        .cloned()
        .collect();
    assert!(
        extras.is_empty(),
        "only the superseded original and closed children are announced, saw {changes:?}"
    );
}

#[test]
fn a_legacy_reassignment_announces_every_child_the_cascade_closes() {
    let (h, events) = recording_stack();
    let session = &h.session;
    let tree = cascade_tree(session, "reassign");
    plant_executing_run(&h.database_path, tree.busy.ticket_id as i64);
    let busy_before = read(session, &tree.busy.identifier);
    let already_before = read(session, &tree.already.identifier);
    let edge_child_before = read(session, &tree.edge_child.identifier);
    let parent_before = read(session, &tree.parent.identifier);

    events.clear();
    let replacement = h
        .core
        .command(
            "ticket.reassign",
            &reassignment_of(
                tree.parent.ticket_id,
                parent_before.version,
                "legacy-reassign-announce",
            ),
        )
        .expect("the legacy reassignment lands");
    let replacement_id = replacement["id"]
        .as_u64()
        .expect("the reassignment returns the replacement");

    assert_eq!(read(session, &tree.parent.identifier).state, "Superseded");
    assert_eq!(read(session, &tree.quiet.identifier).state, "Cancelled");
    assert_eq!(
        read(session, &tree.quiet_child.identifier).state,
        "Cancelled"
    );
    assert_eq!(read(session, &tree.edge.identifier).state, "Cancelled");
    assert_eq!(read(session, &tree.busy.identifier), busy_before);
    assert_eq!(read(session, &tree.busy_child.identifier).state, "Ready");
    assert_eq!(read(session, &tree.already.identifier), already_before);
    assert_eq!(
        read(session, &tree.edge_child.identifier),
        edge_child_before
    );
    let replacement_key = format!("CORE-{}", replacement["number"].as_u64().expect("a number"));
    assert_eq!(
        read(session, &replacement_key).state,
        "Draft",
        "the replacement is a new draft, not a closed child"
    );
    assert_reassignment_announces_closed_children(
        &events,
        tree.parent.ticket_id,
        &closed_by_reassignment(&tree),
        &left_open_by_cancel(&tree),
    );
    let created = events.named("ticket.created");
    assert_eq!(
        created.len(),
        1,
        "only the replacement is created, saw {created:?}"
    );
    assert_eq!(created[0]["id"], json!(replacement_id));
    assert_eq!(created[0]["predecessor_id"], json!(tree.parent.ticket_id));
    assert!(
        !events
            .state_changes()
            .iter()
            .any(|(id, _)| *id == replacement_id),
        "the replacement is not announced as a state change"
    );
}

#[test]
fn a_refused_legacy_reassignment_publishes_no_child_state_change() {
    let (h, events) = recording_stack();
    let session = &h.session;
    close_with_parent(session);
    let parent = save(
        session,
        "refused-reassign-parent",
        json!({ "project_id": CORE, "title": "Parent", "state": "Ready" }),
    );
    let child = child_of(session, &parent, "refused-reassign-child", "Ready");
    let parent_before = read(session, &parent.identifier);
    let child_before = read(session, &child.identifier);

    events.clear();
    let error = h
        .core
        .command(
            "ticket.reassign",
            &reassignment_of(parent.ticket_id, 0, "legacy-reassign-stale"),
        )
        .expect_err("a stale reassignment is refused");

    assert_eq!(error.code, kanban_dto::ErrorCode::StaleVersion);
    assert_eq!(read(session, &parent.identifier), parent_before);
    assert_eq!(read(session, &child.identifier), child_before);
    assert!(
        events.state_changes().is_empty(),
        "a refused reassignment publishes no state change, saw {:?}",
        events.state_changes()
    );
    assert!(
        events.named("ticket.created").is_empty(),
        "a refused reassignment creates no replacement"
    );

    h.core
        .command(
            "ticket.reassign",
            &reassignment_of(
                parent.ticket_id,
                parent_before.version,
                "legacy-reassign-after-refusal",
            ),
        )
        .expect("the later reassignment lands");
    assert_eq!(read(session, &parent.identifier).state, "Superseded");
    assert_eq!(read(session, &child.identifier).state, "Cancelled");
    assert_reassignment_announces_closed_children(
        &events,
        parent.ticket_id,
        &[child.ticket_id],
        &[],
    );
}
