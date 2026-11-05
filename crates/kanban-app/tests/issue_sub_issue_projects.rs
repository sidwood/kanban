//! Sub-issues that live in a different project from their parent, as
//! Linear allows: creating one with an explicit project, the milestone
//! a new sub-issue inherits, re-parenting across projects, cycles that
//! cross projects, and how the parent's reads, the parent filter, and
//! parent auto-close see a child from another project.

use std::sync::Arc;

use kanban_app::{
    CommentIssueTargets, CommentStore, Core, IssueStore, IssueStoreCommentTargets, OperatorSession,
    ProjectStore,
};
use kanban_domain::ProjectRegistration;
use kanban_dto::{
    ApiError, ErrorCode, IssueGetQuery, IssueListResponse, IssueMilestoneRecord, IssueRecord,
    IssueSaveResponse, IssueSubIssueProgress, TimelineEntityKind, TimelineEntityRef,
    TimelineEventKind,
};
use kanban_storage::{
    Database, SqliteCommentStore, SqliteEvidenceStore, SqliteIssueStore, SqlitePlanStore,
    SqliteProjectStore, SqliteSpecStore, SqliteTicketStore,
};
use serde_json::{Value, json};

mod common;

use common::{harness, mutation};

/// The fixture's own Project, registered by the shared harness.
const CORE: u64 = 1;
/// A second Project registered beside it.
const EDGE: u64 = 2;

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

fn register_edge_project(database: &Database) {
    let registration = ProjectRegistration::new(
        "EDGE",
        "Edge tooling",
        "/repositories/edge",
        "/workspaces/edge.seed",
        "main",
        "edge.seed",
        Some("edge-main"),
        None,
    )
    .expect("the edge registration validates");
    SqliteProjectStore::new(database)
        .create(&registration, &|id| {
            kanban_app::TimelineEnvelope::project(
                id.value(),
                TimelineEventKind::Transition,
                Some(TimelineEntityRef {
                    kind: TimelineEntityKind::Project,
                    id: id.value().to_string(),
                }),
                json!({ "action": "registered" }),
            )
        })
        .expect("the edge Project lands");
}

struct TwoProjects {
    _dir: tempfile::TempDir,
    session: OperatorSession,
}

/// CORE and EDGE, both serving the operator issue tools.
fn two_projects() -> TwoProjects {
    let mut h = harness();
    let attachments = h._dir.path().join("attachments");
    register_issue_stack(&mut h.core, &h.database, &attachments);
    register_edge_project(&h.database);
    TwoProjects {
        session: OperatorSession::new(Arc::new(h.core)),
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

fn try_save(
    session: &OperatorSession,
    version: u64,
    key: &str,
    fields: Value,
) -> Result<IssueSaveResponse, ApiError> {
    session
        .call("issue.save", &payload(version, key, fields))
        .map(|value| serde_json::from_value(value).expect("save shape"))
}

fn save(session: &OperatorSession, version: u64, key: &str, fields: Value) -> IssueSaveResponse {
    try_save(session, version, key, fields).expect("issue.save")
}

fn update(session: &OperatorSession, identifier: &str, key: &str, fields: Value) {
    let mut fields = fields;
    fields["identifier"] = json!(identifier);
    save(session, read(session, identifier).version, key, fields);
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

fn list(session: &OperatorSession, query: Value) -> Vec<String> {
    let response: IssueListResponse = serde_json::from_value(
        session
            .call("issue.list", &query)
            .expect("issue.list answers"),
    )
    .expect("list shape");
    let mut identifiers: Vec<String> = response
        .issues
        .into_iter()
        .map(|issue| issue.identifier)
        .collect();
    identifiers.sort();
    identifiers
}

fn child_keys(record: &IssueRecord) -> Vec<&str> {
    record
        .children
        .iter()
        .flatten()
        .map(|child| child.identifier.as_str())
        .collect()
}

fn milestone(session: &OperatorSession, project_id: u64, name: &str) -> u64 {
    let created: IssueMilestoneRecord = serde_json::from_value(
        session
            .call(
                "issue.create_milestone",
                &json!({
                    "mutation": mutation(0, format!("milestone-{project_id}-{name}")),
                    "project_id": project_id,
                    "name": name,
                }),
            )
            .expect("the milestone lands"),
    )
    .expect("milestone shape");
    created.id
}

fn enable_parent_auto_close(session: &OperatorSession, project_id: u64) {
    session
        .call(
            "issue.update_project_settings",
            &json!({
                "mutation": mutation(0, format!("auto-close-{project_id}")),
                "project_id": project_id,
                "auto_close_sub_issues": true,
            }),
        )
        .expect("parent auto-close is enabled");
}

#[test]
fn a_new_sub_issue_may_name_another_project() {
    let h = two_projects();
    let s = &h.session;
    let parent = save(
        s,
        0,
        "parent",
        json!({ "project_id": CORE, "title": "Parent" }),
    );

    let child = save(
        s,
        0,
        "child-elsewhere",
        json!({
            "project_id": EDGE,
            "title": "Child in the edge project",
            "parent_id": parent.identifier,
        }),
    );

    assert_eq!(
        child.identifier, "EDGE-1",
        "the child takes the key of its own project"
    );
    let read_child = read(s, &child.identifier);
    assert_eq!(read_child.project_id, EDGE);
    assert_eq!(read_child.parent.as_deref(), Some("CORE-1"));
    let read_parent = read(s, &parent.identifier);
    assert_eq!(child_keys(&read_parent), vec!["EDGE-1"]);
    assert_eq!(
        read_parent.sub_issues,
        Some(IssueSubIssueProgress { done: 0, total: 1 })
    );
}

#[test]
fn a_new_sub_issue_in_the_parents_project_inherits_its_milestone() {
    let h = two_projects();
    let s = &h.session;
    let core_milestone = milestone(s, CORE, "PL1 Staging environment");
    let parent = save(
        s,
        0,
        "parent",
        json!({ "project_id": CORE, "title": "Parent", "milestone_id": core_milestone }),
    );

    let defaulted = save(
        s,
        0,
        "child-defaulted",
        json!({ "title": "Child with no project", "parent_id": parent.identifier }),
    );
    let explicit = save(
        s,
        0,
        "child-explicit",
        json!({
            "project_id": CORE,
            "title": "Child naming the parent's project",
            "parent_id": parent.identifier,
        }),
    );

    for key in [&defaulted.identifier, &explicit.identifier] {
        let record = read(s, key);
        assert_eq!(
            record.project_id, CORE,
            "{key} stays in the parent's project"
        );
        assert_eq!(
            record.milestone_id,
            Some(core_milestone),
            "{key} inherits the parent's milestone"
        );
    }
}

#[test]
fn a_new_sub_issue_in_another_project_leaves_the_parents_milestone_behind() {
    let h = two_projects();
    let s = &h.session;
    let core_milestone = milestone(s, CORE, "PL1 Staging environment");
    let edge_milestone = milestone(s, EDGE, "OP8 Usable ops");
    let parent = save(
        s,
        0,
        "parent",
        json!({ "project_id": CORE, "title": "Parent", "milestone_id": core_milestone }),
    );

    let bare = save(
        s,
        0,
        "child-bare",
        json!({
            "project_id": EDGE,
            "title": "Child with no milestone",
            "parent_id": parent.identifier,
        }),
    );
    let placed = save(
        s,
        0,
        "child-placed",
        json!({
            "project_id": EDGE,
            "title": "Child on its own project's milestone",
            "parent_id": parent.identifier,
            "milestone_id": edge_milestone,
        }),
    );

    assert_eq!(
        read(s, &bare.identifier).milestone_id,
        None,
        "a milestone of the parent's project cannot follow the child elsewhere"
    );
    assert_eq!(
        read(s, &placed.identifier).milestone_id,
        Some(edge_milestone)
    );
}

#[test]
fn a_new_sub_issue_elsewhere_refuses_the_parents_milestone_by_name() {
    let h = two_projects();
    let s = &h.session;
    let core_milestone = milestone(s, CORE, "PL1 Staging environment");
    let parent = save(
        s,
        0,
        "parent",
        json!({ "project_id": CORE, "title": "Parent", "milestone_id": core_milestone }),
    );

    let error = try_save(
        s,
        0,
        "child-wrong-milestone",
        json!({
            "project_id": EDGE,
            "title": "Child naming the parent's milestone",
            "parent_id": parent.identifier,
            "milestone_id": core_milestone,
        }),
    )
    .expect_err("a milestone of another project is refused");

    assert_eq!(error.code, ErrorCode::InvalidRequest);
    assert_eq!(error.field.as_deref(), Some("milestone_id"));
    assert!(
        list(s, json!({ "project_id": EDGE })).is_empty(),
        "the refused save leaves no issue behind"
    );
    assert_eq!(read(s, &parent.identifier).children, None);
}

#[test]
fn an_issue_may_move_under_a_parent_in_another_project() {
    let h = two_projects();
    let s = &h.session;
    let edge_milestone = milestone(s, EDGE, "OP8 Usable ops");
    let core_parent = save(
        s,
        0,
        "core-parent",
        json!({ "project_id": CORE, "title": "Core parent" }),
    );
    let edge_parent = save(
        s,
        0,
        "edge-parent",
        json!({ "project_id": EDGE, "title": "Edge parent" }),
    );
    let moving = save(
        s,
        0,
        "moving",
        json!({
            "project_id": EDGE,
            "title": "Issue that changes parent",
            "milestone_id": edge_milestone,
        }),
    );

    update(
        s,
        &moving.identifier,
        "under-core",
        json!({ "parent_id": core_parent.identifier }),
    );
    let moved = read(s, &moving.identifier);
    assert_eq!(
        moved.parent.as_deref(),
        Some(core_parent.identifier.as_str())
    );
    assert_eq!(
        moved.project_id, EDGE,
        "re-parenting never moves the project"
    );
    assert_eq!(
        moved.milestone_id,
        Some(edge_milestone),
        "the child keeps its own project's milestone"
    );
    assert_eq!(
        child_keys(&read(s, &core_parent.identifier)),
        vec![moving.identifier.as_str()]
    );

    update(
        s,
        &moving.identifier,
        "under-edge",
        json!({ "parent_id": edge_parent.identifier }),
    );
    let left = read(s, &core_parent.identifier);
    assert_eq!(left.children, None, "the old parent lets the child go");
    assert_eq!(left.sub_issues, None);
    assert_eq!(
        child_keys(&read(s, &edge_parent.identifier)),
        vec![moving.identifier.as_str()]
    );

    update(
        s,
        &moving.identifier,
        "top-level",
        json!({ "parent_id": null }),
    );
    assert_eq!(read(s, &moving.identifier).parent, None);
    assert_eq!(read(s, &edge_parent.identifier).children, None);
}

#[test]
fn a_cycle_through_another_project_is_refused() {
    let h = two_projects();
    let s = &h.session;
    let top = save(s, 0, "top", json!({ "project_id": CORE, "title": "Top" }));
    let middle = save(
        s,
        0,
        "middle",
        json!({ "project_id": EDGE, "title": "Middle", "parent_id": top.identifier }),
    );
    let bottom = save(
        s,
        0,
        "bottom",
        json!({ "project_id": CORE, "title": "Bottom", "parent_id": middle.identifier }),
    );

    let error = try_save(
        s,
        read(s, &top.identifier).version,
        "cycle",
        json!({ "identifier": top.identifier, "parent_id": bottom.identifier }),
    )
    .expect_err("an issue cannot become its own ancestor through another project");

    assert_eq!(error.code, ErrorCode::InvalidRequest);
    assert_eq!(error.field.as_deref(), Some("parent_id"));
    assert_eq!(read(s, &top.identifier).parent, None);
}

#[test]
fn progress_and_children_count_sub_issues_from_every_project() {
    let h = two_projects();
    let s = &h.session;
    let parent = save(
        s,
        0,
        "parent",
        json!({ "project_id": CORE, "title": "Parent" }),
    );
    for (key, project, title, state) in [
        ("core-done", CORE, "Done here", "Done"),
        ("edge-open", EDGE, "Open elsewhere", "Active"),
        ("edge-cancelled", EDGE, "Cancelled elsewhere", "Cancelled"),
    ] {
        save(
            s,
            0,
            key,
            json!({
                "project_id": project,
                "title": title,
                "state": state,
                "parent_id": parent.identifier,
            }),
        );
    }

    let record = read(s, &parent.identifier);
    assert_eq!(
        record.sub_issues,
        Some(IssueSubIssueProgress { done: 2, total: 3 }),
        "a closed child counts as done whichever project holds it"
    );
    let children: Vec<(&str, &str)> = record
        .children
        .iter()
        .flatten()
        .map(|child| (child.identifier.as_str(), child.state.as_str()))
        .collect();
    assert_eq!(
        children,
        vec![
            ("CORE-2", "Done"),
            ("EDGE-1", "Active"),
            ("EDGE-2", "Cancelled"),
        ],
        "children list in manual order with their own project's state"
    );
}

#[test]
fn the_parent_filter_finds_sub_issues_in_every_project() {
    let h = two_projects();
    let s = &h.session;
    let parent = save(
        s,
        0,
        "parent",
        json!({ "project_id": CORE, "title": "Parent" }),
    );
    save(
        s,
        0,
        "core-child",
        json!({ "project_id": CORE, "title": "Core child", "parent_id": parent.identifier }),
    );
    save(
        s,
        0,
        "edge-child",
        json!({ "project_id": EDGE, "title": "Edge child", "parent_id": parent.identifier }),
    );
    save(
        s,
        0,
        "edge-stranger",
        json!({ "project_id": EDGE, "title": "Unrelated" }),
    );

    assert_eq!(
        list(s, json!({ "parent_id": parent.identifier })),
        vec!["CORE-2", "EDGE-1"]
    );
    assert_eq!(
        list(s, json!({ "parent_id": parent.ticket_id.to_string() })),
        vec!["CORE-2", "EDGE-1"],
        "the numeric parent id filters the same way"
    );
    assert_eq!(
        list(
            s,
            json!({ "parent_id": parent.identifier, "project_id": EDGE })
        ),
        vec!["EDGE-1"],
        "a project filter narrows the children to that project"
    );
    assert_eq!(
        list(
            s,
            json!({ "parent_id": parent.identifier, "team_id": CORE })
        ),
        vec!["CORE-2"]
    );
}

#[test]
fn the_parents_project_decides_auto_close_for_a_child_elsewhere() {
    let h = two_projects();
    let s = &h.session;
    enable_parent_auto_close(s, EDGE);
    let left_open = save(
        s,
        0,
        "left-open",
        json!({ "project_id": CORE, "title": "Parent in a project without auto-close", "state": "Active" }),
    );
    let lone = save(
        s,
        0,
        "lone-child",
        json!({ "project_id": EDGE, "title": "Child", "parent_id": left_open.identifier, "state": "Active" }),
    );
    update(s, &lone.identifier, "lone-done", json!({ "state": "Done" }));
    assert_eq!(
        read(s, &left_open.identifier).state,
        "Active",
        "the child's project setting does not close a parent elsewhere"
    );

    enable_parent_auto_close(s, CORE);
    let closing = save(
        s,
        0,
        "closing",
        json!({ "project_id": CORE, "title": "Parent in a project with auto-close", "state": "Active" }),
    );
    let finisher = save(
        s,
        0,
        "finisher",
        json!({ "project_id": EDGE, "title": "Child", "parent_id": closing.identifier, "state": "Active" }),
    );
    update(
        s,
        &finisher.identifier,
        "finisher-done",
        json!({ "state": "Done" }),
    );
    let closed = read(s, &closing.identifier);
    assert_eq!(
        closed.state, "Done",
        "the parent's project closes it when its last child elsewhere finishes"
    );
    assert!(closed.completed_at.is_some());
}
