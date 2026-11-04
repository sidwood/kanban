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

struct NameCaseHarness {
    _dir: tempfile::TempDir,
    session: OperatorSession,
    database: Database,
}

/// An operator session over a fresh database holding project 1 and a
/// second project, so scope checks have somewhere else to look.
fn session() -> NameCaseHarness {
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
    NameCaseHarness {
        session: OperatorSession::new(Arc::new(h.core)),
        database: h.database,
        _dir: h._dir,
    }
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
    let h = session();
    let session = &h.session;
    create_label(session, "web", 1, "web", None).expect("the first spelling lands");
    let before = labels(session, 1);

    for (key, variant, group) in [
        ("web-upper", "WEB", None),
        ("web-title", "Web", None),
        ("web-grouped", "wEb", Some("Area")),
    ] {
        let error = create_label(session, key, 1, variant, group)
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
        labels(session, 1),
        before,
        "a refused label lands nothing, not even its new group"
    );
}

#[test]
fn a_milestone_differing_only_in_case_is_refused_and_changes_nothing() {
    let h = session();
    let session = &h.session;
    create_milestone(session, "m1", 1, "M1 Foundations").expect("the first spelling lands");
    let before = milestone_names(session, 1);

    for (key, variant) in [
        ("m1-lower", "m1 foundations"),
        ("m1-upper", "M1 FOUNDATIONS"),
    ] {
        let error = create_milestone(session, key, 1, variant)
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

    assert_eq!(milestone_names(session, 1), before);
}

#[test]
fn an_exact_repeat_still_conflicts() {
    let h = session();
    let session = &h.session;
    create_label(session, "web", 1, "web", None).expect("the label lands");
    create_milestone(session, "m1", 1, "M1 Foundations").expect("the milestone lands");

    let label =
        create_label(session, "web-again", 1, "web", None).expect_err("an exact repeat is refused");
    assert_eq!(label.code, ErrorCode::Conflict);
    assert_eq!(label.field.as_deref(), Some("name"));

    let milestone = create_milestone(session, "m1-again", 1, "M1 Foundations")
        .expect_err("an exact repeat is refused");
    assert_eq!(milestone.code, ErrorCode::Conflict);
    assert_eq!(milestone.field.as_deref(), Some("name"));
}

#[test]
fn case_variants_stay_free_in_another_project() {
    let h = session();
    let session = &h.session;
    create_label(session, "web", 1, "web", None).expect("the label lands");
    create_milestone(session, "m1", 1, "M1 Foundations").expect("the milestone lands");

    create_label(session, "web-edge", 2, "WEB", None)
        .expect("another project's labels are another scope");
    create_milestone(session, "m1-edge", 2, "m1 foundations")
        .expect("another project's milestones are another scope");
}

#[test]
fn letter_case_follows_unicode_case_folding() {
    let h = session();
    let session = &h.session;
    create_label(session, "cafe", 1, "café", None).expect("the label lands");
    create_milestone(session, "strasse", 1, "Straße").expect("the milestone lands");

    let label = create_label(session, "cafe-upper", 1, "CAFÉ", None)
        .expect_err("non-ASCII letters fold too");
    assert_eq!(label.code, ErrorCode::InvalidRequest);
    assert_eq!(label.field.as_deref(), Some("name"));

    let milestone = create_milestone(session, "strasse-upper", 1, "STRASSE")
        .expect_err("full case folding treats ß as ss");
    assert_eq!(milestone.code, ErrorCode::InvalidRequest);
    assert_eq!(milestone.field.as_deref(), Some("name"));
}

fn save_issue(session: &OperatorSession, key: &str, fields: Value) -> Result<Value, ApiError> {
    let mut payload = fields;
    let version = match payload["identifier"].as_str() {
        Some(identifier) => read_issue(session, identifier)["version"].clone(),
        None => json!(0),
    };
    payload["mutation"] = json!({ "optimistic_version": version, "idempotency_key": key });
    session.call("issue.save", &payload)
}

fn read_issue(session: &OperatorSession, identifier: &str) -> Value {
    session
        .call("issue.get", &json!({ "identifier": identifier }))
        .expect("issue.get")
}

fn issue_labels(session: &OperatorSession, identifier: &str) -> Vec<String> {
    read_issue(session, identifier)["labels"]
        .as_array()
        .expect("labels array")
        .iter()
        .map(|label| label.as_str().expect("label name").to_owned())
        .collect()
}

fn new_issue(session: &OperatorSession, key: &str, title: &str) -> String {
    save_issue(session, key, json!({ "project_id": 1, "title": title }))
        .expect("the issue lands")["identifier"]
        .as_str()
        .expect("identifier")
        .to_owned()
}

/// A label row written straight to storage, the way a database built
/// before the case rule may already hold one.
fn insert_legacy_label(database: &Database, name: &str) {
    database
        .connection()
        .execute(
            "INSERT INTO issue_labels (project_id, name, color) VALUES (1, ?1, '#336699')",
            [name],
        )
        .expect("the legacy label row lands");
}

fn retire_label(session: &OperatorSession, key: &str, name: &str) -> Result<Value, ApiError> {
    session.call(
        "issue.retire_label",
        &json!({
            "mutation": mutation(0, key),
            "project_id": 1,
            "name": name,
        }),
    )
}

#[test]
fn save_finds_a_label_whatever_case_names_it() {
    let h = session();
    let session = &h.session;
    create_label(session, "web", 1, "web", None).expect("the label lands");
    let issue = new_issue(session, "host", "Case-blind labels");

    save_issue(
        session,
        "add-upper",
        json!({ "identifier": issue, "add_labels": ["WEB"] }),
    )
    .expect("a label added in another case lands");
    assert_eq!(
        issue_labels(session, &issue),
        vec!["web".to_owned()],
        "the issue holds the stored spelling"
    );

    save_issue(
        session,
        "remove-title",
        json!({ "identifier": issue, "remove_labels": ["Web"] }),
    )
    .expect("a label removed in another case lands");
    assert!(issue_labels(session, &issue).is_empty());

    let created = save_issue(
        session,
        "create-with-upper",
        json!({ "project_id": 1, "title": "Labelled at birth", "add_labels": ["WEB"] }),
    )
    .expect("a new issue takes a label in another case");
    assert_eq!(
        issue_labels(session, created["identifier"].as_str().unwrap()),
        vec!["web".to_owned()]
    );
}

#[test]
fn a_single_select_group_counts_one_label_once_whatever_its_case() {
    let h = session();
    let session = &h.session;
    session
        .call(
            "issue.create_label",
            &json!({
                "mutation": mutation(0, "tier"),
                "project_id": 1,
                "name": "tier-simple",
                "color": "#336699",
                "group": "Model tier",
                "group_single_select": true,
            }),
        )
        .expect("the tier label lands");
    let issue = new_issue(session, "tier-host", "One tier");

    save_issue(
        session,
        "tier-twice",
        json!({ "identifier": issue, "add_labels": ["tier-simple", "TIER-SIMPLE"] }),
    )
    .expect("two spellings of one label are one label, not two from the group");
    assert_eq!(
        issue_labels(session, &issue),
        vec!["tier-simple".to_owned()]
    );
}

#[test]
fn retire_finds_a_label_whatever_case_names_it() {
    let h = session();
    let session = &h.session;
    create_label(session, "legacy", 1, "legacy", None).expect("the label lands");

    let retired = retire_label(session, "retire-upper", "LEGACY")
        .expect("a label retired in another case is found");
    assert_eq!(retired["name"], json!("legacy"));
    assert_eq!(retired["retired"], json!(true));

    let again = retire_label(session, "retire-again", "Legacy")
        .expect_err("the found label is already retired");
    assert_eq!(again.code, ErrorCode::InvalidRequest);
    assert_eq!(again.field.as_deref(), Some("name"));
}

#[test]
fn an_unknown_label_is_still_unknown_in_any_case() {
    let h = session();
    let session = &h.session;
    let issue = new_issue(session, "unknown-host", "No such label");

    let error = save_issue(
        session,
        "unknown-add",
        json!({ "identifier": issue, "add_labels": ["NOPE"] }),
    )
    .expect_err("an unknown label is refused");
    assert_eq!(error.code, ErrorCode::InvalidRequest);
    assert_eq!(error.field.as_deref(), Some("add_labels"));
    assert!(
        error.message.contains("unknown label `NOPE`"),
        "{}",
        error.message
    );

    let missing = retire_label(session, "unknown-retire", "NOPE")
        .expect_err("an unknown label cannot be retired");
    assert_eq!(missing.code, ErrorCode::NotFound);
}

#[test]
fn a_stored_case_variant_pair_stays_assignable_by_exact_spelling() {
    let h = session();
    let session = &h.session;
    insert_legacy_label(&h.database, "web");
    insert_legacy_label(&h.database, "WEB");
    let issue = new_issue(session, "pair-host", "Pair holder");

    save_issue(
        session,
        "pair-add-both",
        json!({ "identifier": issue, "add_labels": ["web", "WEB"] }),
    )
    .expect("each exact spelling names its own stored label");
    assert_eq!(
        issue_labels(session, &issue),
        vec!["WEB".to_owned(), "web".to_owned()]
    );

    save_issue(
        session,
        "pair-remove-upper",
        json!({ "identifier": issue, "remove_labels": ["WEB"] }),
    )
    .expect("an exact spelling removes only its own label");
    assert_eq!(issue_labels(session, &issue), vec!["web".to_owned()]);
}

#[test]
fn a_stored_case_variant_pair_refuses_to_guess_and_changes_nothing() {
    let h = session();
    let session = &h.session;
    insert_legacy_label(&h.database, "web");
    insert_legacy_label(&h.database, "WEB");
    let issue = new_issue(session, "guess-host", "Guess holder");
    save_issue(
        session,
        "guess-seed",
        json!({ "identifier": issue, "add_labels": ["web"] }),
    )
    .expect("the exact spelling lands");
    let version = read_issue(session, &issue)["version"].clone();

    for (key, field, change) in [
        ("guess-add", "add_labels", json!({ "add_labels": ["Web"] })),
        (
            "guess-remove",
            "remove_labels",
            json!({ "remove_labels": ["Web"] }),
        ),
        (
            "guess-mixed",
            "add_labels",
            json!({ "title": "Renamed", "add_labels": ["Web"] }),
        ),
    ] {
        let mut payload = change;
        payload["identifier"] = json!(issue);
        let error = save_issue(session, key, payload).expect_err("an ambiguous name is refused");
        assert_eq!(error.code, ErrorCode::InvalidRequest, "{key}");
        assert_eq!(error.field.as_deref(), Some(field), "{key}");
        assert!(
            error.message.contains("`WEB` and `web`"),
            "the refusal names both stored spellings: {}",
            error.message
        );
    }
    let after = read_issue(session, &issue);
    assert_eq!(after["version"], version, "a refused save changes nothing");
    assert_eq!(after["title"], json!("Guess holder"));
    assert_eq!(issue_labels(session, &issue), vec!["web".to_owned()]);

    let retire =
        retire_label(session, "guess-retire", "Web").expect_err("an ambiguous retire is refused");
    assert_eq!(retire.code, ErrorCode::InvalidRequest);
    assert_eq!(retire.field.as_deref(), Some("name"));
    assert!(
        labels(session, 1)
            .iter()
            .any(|(name, _)| name == "web" || name == "WEB"),
        "both stored spellings still list"
    );
    let listed = session
        .call("issue.list_labels", &json!({ "project_id": 1 }))
        .expect("labels list");
    assert!(
        listed["labels"]
            .as_array()
            .unwrap()
            .iter()
            .all(|label| label["retired"] == json!(false)),
        "a refused retire retires nothing"
    );

    retire_label(session, "pair-retire-upper", "WEB")
        .expect("an exact spelling retires only its own label");
}

#[test]
fn the_label_filter_matches_every_stored_case_variant() {
    let h = session();
    let session = &h.session;
    insert_legacy_label(&h.database, "web");
    insert_legacy_label(&h.database, "WEB");
    let lower = new_issue(session, "filter-lower", "Lower holder");
    let upper = new_issue(session, "filter-upper", "Upper holder");
    save_issue(
        session,
        "filter-lower-add",
        json!({ "identifier": lower, "add_labels": ["web"] }),
    )
    .unwrap();
    save_issue(
        session,
        "filter-upper-add",
        json!({ "identifier": upper, "add_labels": ["WEB"] }),
    )
    .unwrap();

    let listed = session
        .call("issue.list", &json!({ "project_id": 1, "label": "Web" }))
        .expect("issue.list");
    let mut found: Vec<String> = listed["issues"]
        .as_array()
        .unwrap()
        .iter()
        .map(|issue| issue["identifier"].as_str().unwrap().to_owned())
        .collect();
    found.sort();
    let mut expected = vec![lower, upper];
    expected.sort();
    assert_eq!(
        found, expected,
        "a filter reads, so it matches every spelling of the name rather than refusing"
    );
}

fn create_grouped_label(
    session: &OperatorSession,
    key: &str,
    name: &str,
    group: &str,
) -> Result<Value, ApiError> {
    session.call(
        "issue.create_label",
        &json!({
            "mutation": mutation(0, key),
            "project_id": 1,
            "name": name,
            "color": "#336699",
            "group": group,
            "group_single_select": true,
        }),
    )
}

#[test]
fn a_label_joins_its_group_whatever_case_names_the_group() {
    let h = session();
    let session = &h.session;
    create_grouped_label(session, "tier-simple", "tier-simple", "Model tier")
        .expect("the group lands with its first label");

    let joined = create_grouped_label(session, "tier-standard", "tier-standard", "model TIER")
        .expect("a label names its group in another case");
    assert_eq!(joined["group"], json!("Model tier"));

    let grouped: Vec<_> = labels(session, 1)
        .into_iter()
        .filter(|(name, _)| name.starts_with("tier-"))
        .collect();
    assert_eq!(
        grouped,
        vec![
            ("tier-simple".to_owned(), Some("Model tier".to_owned())),
            ("tier-standard".to_owned(), Some("Model tier".to_owned())),
        ],
        "one group holds both labels"
    );

    let issue = new_issue(session, "tier-host", "One tier");
    let error = save_issue(
        session,
        "tier-both",
        json!({ "identifier": issue, "add_labels": ["tier-simple", "tier-standard"] }),
    )
    .expect_err("the joined group stays single-select");
    assert_eq!(error.field.as_deref(), Some("add_labels"));
}

#[test]
fn a_stored_case_variant_group_pair_refuses_to_guess() {
    let h = session();
    let session = &h.session;
    h.database
        .connection()
        .execute_batch(
            "INSERT INTO issue_label_groups (project_id, name, single_select)
             VALUES (1, 'Area', 0), (1, 'AREA', 0);",
        )
        .expect("the legacy group rows land");
    let before = labels(session, 1);

    let error = create_grouped_label(session, "area-guess", "web", "area")
        .expect_err("an ambiguous group name is refused");
    assert_eq!(error.code, ErrorCode::InvalidRequest);
    assert_eq!(error.field.as_deref(), Some("group"));
    assert!(
        error.message.contains("`AREA` and `Area`"),
        "{}",
        error.message
    );
    assert_eq!(labels(session, 1), before, "a refused label lands nothing");

    let exact = create_grouped_label(session, "area-exact", "web", "AREA")
        .expect("an exact spelling names its own group");
    assert_eq!(exact["group"], json!("AREA"));
}
