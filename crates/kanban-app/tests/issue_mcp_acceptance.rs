//! MCP acceptance scenarios from `mcp-acceptance-tests.md` (Linear parity).
//! Exercises the installation operator surface through `OperatorSession`.

use std::sync::Arc;

use kanban_app::{Core, IssueStore, OperatorSession};
use kanban_dto::{IssueGetQuery, IssueRecord, IssueSaveResponse};
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
    core.register_issue_operator(tickets, projects, specs, issues)
        .expect("issue operator registers");
}

#[test]
fn acceptance_create_and_read_issue_returns_key_and_fields() {
    let mut h = harness();
    let attachments = h._dir.path().join("attachments");
    std::fs::create_dir_all(&attachments).expect("attachments dir");
    register_issue_stack(&mut h.core, &h.database, &attachments);
    let core = Arc::new(h.core);
    let session = OperatorSession::new(core);

    let save = json!({
        "mutation": mutation(0, "acceptance-create"),
        "project_id": 1,
        "title": "Checklist issue",
        "description": "- [ ] Ship Linear parity\n",
        "state": "Draft",
        "priority": 2,
        "due_date": "2026-12-01",
    });
    let saved: IssueSaveResponse = serde_json::from_value(
        session
            .call("issue.save", &save)
            .expect("save succeeds"),
    )
    .expect("save response shape");
    assert!(saved.identifier.contains('-'));
    assert!(saved.url.starts_with("kanban://issue/"));

    let read: IssueRecord = serde_json::from_value(
        session
            .call(
                "issue.get",
                &serde_json::to_value(IssueGetQuery {
                    identifier: saved.identifier.clone(),
                })
                .unwrap(),
            )
            .expect("get succeeds"),
    )
    .expect("issue record shape");
    assert_eq!(read.identifier, saved.identifier);
    assert!(read.description.contains("Ship Linear parity"));
    assert_eq!(read.state, "Draft");
    assert_eq!(read.priority, 2);
}

#[test]
fn acceptance_free_state_move_appends_without_lifecycle_enforcement() {
    let mut h = harness();
    let attachments = h._dir.path().join("attachments");
    std::fs::create_dir_all(&attachments).expect("attachments dir");
    register_issue_stack(&mut h.core, &h.database, &attachments);
    let core = Arc::new(h.core);
    let session = OperatorSession::new(core);

    let saved: IssueSaveResponse = serde_json::from_value(
        session
            .call(
                "issue.save",
                &save_payload(
                    0,
                    "acceptance-state",
                    json!({
                        "project_id": 1,
                        "title": "State hop",
                        "state": "Draft",
                    }),
                ),
            )
            .expect("create succeeds"),
    )
    .unwrap();

    session
        .call(
            "issue.save",
            &save_payload(
                2,
                "acceptance-state-done",
                json!({
                    "identifier": saved.identifier,
                    "state": "Done",
                }),
            ),
        )
        .expect("done transition succeeds");

    let read: IssueRecord = serde_json::from_value(
        session
            .call(
                "issue.get",
                &serde_json::to_value(IssueGetQuery {
                    identifier: saved.identifier,
                })
                .unwrap(),
            )
            .expect("get succeeds"),
    )
    .unwrap();
    assert_eq!(read.state, "Done");
    assert_eq!(read.state_type, "completed");
}

#[test]
fn acceptance_retired_label_is_rejected() {
    let mut h = harness();
    let attachments = h._dir.path().join("attachments");
    std::fs::create_dir_all(&attachments).expect("attachments dir");
    register_issue_stack(&mut h.core, &h.database, &attachments);
    let conn = rusqlite::Connection::open(&h.database_path).expect("database opens");
    conn.execute(
        "UPDATE issue_labels SET retired_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
         WHERE project_id = 1 AND name = 'task'",
        [],
    )
    .expect("retire label");
    drop(conn);

    let core = Arc::new(h.core);
    let session = OperatorSession::new(core);
    let saved: IssueSaveResponse = serde_json::from_value(
        session
            .call(
                "issue.save",
                &save_payload(
                    0,
                    "acceptance-label",
                    json!({
                        "project_id": 1,
                        "title": "Label test",
                    }),
                ),
            )
            .expect("create succeeds"),
    )
    .unwrap();

    let error = session
        .call(
            "issue.save",
            &save_payload(
                1,
                "acceptance-label-add",
                json!({
                    "identifier": saved.identifier,
                    "add_labels": ["task"],
                }),
            ),
        )
        .expect_err("retired label refused");
    assert_eq!(error.code, kanban_dto::ErrorCode::InvalidRequest);
}
