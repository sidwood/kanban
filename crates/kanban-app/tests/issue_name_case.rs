//! Label and milestone names are unique within their project ignoring
//! letter case, the way Linear matches them. A name that differs from
//! a stored one only in letter case is refused, and by-name lookups
//! find a stored name whatever case the caller spells it in.

use std::sync::Arc;

use kanban_app::{
    CommentIssueTargets, CommentStore, Core, IssueStore, IssueStoreCommentTargets, OperatorSession,
    ProjectStore, TimelineEnvelope,
};
use kanban_domain::ProjectRegistration;
use kanban_dto::{ApiError, ErrorCode, TimelineEntityKind, TimelineEntityRef, TimelineEventKind};
use kanban_storage::{
    Database, SqliteCommentStore, SqliteEvidenceStore, SqliteIssueStore, SqlitePlanStore,
    SqliteProjectStore, SqliteSpecStore, SqliteTicketStore,
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

/// An operator session over a fresh database holding project 1 and a
/// second project, so scope checks have somewhere else to look.
fn session() -> (tempfile::TempDir, OperatorSession) {
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
    .expect("the edge registration validates");
    projects
        .create(&edge, &|id| {
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
    (h._dir, OperatorSession::new(Arc::new(h.core)))
}

fn create_label(
    session: &OperatorSession,
    key: &str,
    project_id: u64,
    name: &str,
    group: Option<&str>,
) -> Result<Value, ApiError> {
    let mut payload = json!({
        "mutation": mutation(0, key),
        "project_id": project_id,
        "name": name,
        "color": "#336699",
    });
    if let Some(group) = group {
        payload["group"] = json!(group);
    }
    session.call("issue.create_label", &payload)
}

fn create_milestone(
    session: &OperatorSession,
    key: &str,
    project_id: u64,
    name: &str,
) -> Result<Value, ApiError> {
    session.call(
        "issue.create_milestone",
        &json!({
            "mutation": mutation(0, key),
            "project_id": project_id,
            "name": name,
        }),
    )
}

/// Every label of one project as `(name, group)`, in listing order.
fn labels(session: &OperatorSession, project_id: u64) -> Vec<(String, Option<String>)> {
    let listed = session
        .call("issue.list_labels", &json!({ "project_id": project_id }))
        .expect("labels list");
    listed["labels"]
        .as_array()
        .expect("labels array")
        .iter()
        .map(|label| {
            (
                label["name"].as_str().expect("label name").to_owned(),
                label["group"].as_str().map(str::to_owned),
            )
        })
        .collect()
}

fn milestone_names(session: &OperatorSession, project_id: u64) -> Vec<String> {
    let listed = session
        .call(
            "issue.list_milestones",
            &json!({ "project_id": project_id }),
        )
        .expect("milestones list");
    listed["milestones"]
        .as_array()
        .expect("milestones array")
        .iter()
        .map(|milestone| milestone["name"].as_str().expect("name").to_owned())
        .collect()
}

#[test]
fn a_label_differing_only_in_case_is_refused_and_changes_nothing() {
    let (_dir, session) = session();
    create_label(&session, "web", 1, "web", None).expect("the first spelling lands");
    let before = labels(&session, 1);

    for (key, variant, group) in [
        ("web-upper", "WEB", None),
        ("web-title", "Web", None),
        ("web-grouped", "wEb", Some("Area")),
    ] {
        let error = create_label(&session, key, 1, variant, group)
            .expect_err("a case variant of a stored label is refused");
        assert_eq!(error.code, ErrorCode::InvalidRequest, "{variant}");
        assert_eq!(error.field.as_deref(), Some("name"), "{variant}");
        assert!(
            error.message.contains(&format!("`{variant}`")) && error.message.contains("`web`"),
            "the refusal names both spellings: {}",
            error.message
        );
    }

    assert_eq!(
        labels(&session, 1),
        before,
        "a refused label lands nothing, not even its new group"
    );
}

#[test]
fn a_milestone_differing_only_in_case_is_refused_and_changes_nothing() {
    let (_dir, session) = session();
    create_milestone(&session, "m1", 1, "M1 Foundations").expect("the first spelling lands");
    let before = milestone_names(&session, 1);

    for (key, variant) in [
        ("m1-lower", "m1 foundations"),
        ("m1-upper", "M1 FOUNDATIONS"),
    ] {
        let error = create_milestone(&session, key, 1, variant)
            .expect_err("a case variant of a stored milestone is refused");
        assert_eq!(error.code, ErrorCode::InvalidRequest, "{variant}");
        assert_eq!(error.field.as_deref(), Some("name"), "{variant}");
        assert!(
            error.message.contains(&format!("`{variant}`"))
                && error.message.contains("`M1 Foundations`"),
            "the refusal names both spellings: {}",
            error.message
        );
    }

    assert_eq!(milestone_names(&session, 1), before);
}

#[test]
fn an_exact_repeat_still_conflicts() {
    let (_dir, session) = session();
    create_label(&session, "web", 1, "web", None).expect("the label lands");
    create_milestone(&session, "m1", 1, "M1 Foundations").expect("the milestone lands");

    let label = create_label(&session, "web-again", 1, "web", None)
        .expect_err("an exact repeat is refused");
    assert_eq!(label.code, ErrorCode::Conflict);
    assert_eq!(label.field.as_deref(), Some("name"));

    let milestone = create_milestone(&session, "m1-again", 1, "M1 Foundations")
        .expect_err("an exact repeat is refused");
    assert_eq!(milestone.code, ErrorCode::Conflict);
    assert_eq!(milestone.field.as_deref(), Some("name"));
}

#[test]
fn case_variants_stay_free_in_another_project() {
    let (_dir, session) = session();
    create_label(&session, "web", 1, "web", None).expect("the label lands");
    create_milestone(&session, "m1", 1, "M1 Foundations").expect("the milestone lands");

    create_label(&session, "web-edge", 2, "WEB", None)
        .expect("another project's labels are another scope");
    create_milestone(&session, "m1-edge", 2, "m1 foundations")
        .expect("another project's milestones are another scope");
}

#[test]
fn letter_case_follows_unicode_case_folding() {
    let (_dir, session) = session();
    create_label(&session, "cafe", 1, "café", None).expect("the label lands");
    create_milestone(&session, "strasse", 1, "Straße").expect("the milestone lands");

    let label = create_label(&session, "cafe-upper", 1, "CAFÉ", None)
        .expect_err("non-ASCII letters fold too");
    assert_eq!(label.code, ErrorCode::InvalidRequest);
    assert_eq!(label.field.as_deref(), Some("name"));

    let milestone = create_milestone(&session, "strasse-upper", 1, "STRASSE")
        .expect_err("full case folding treats ß as ss");
    assert_eq!(milestone.code, ErrorCode::InvalidRequest);
    assert_eq!(milestone.field.as_deref(), Some("name"));
}
