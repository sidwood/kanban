//! Migration 0017 reunifies the split legacy scopes the earlier
//! commands wrote: rows keep their identities, times, and facts, and
//! the one Project they can only belong to takes them back
//! (KAN-T79-AC3).

use kanban_app::{CommentStore, EvidenceStore, ProjectStore, TimelineEnvelope};
use kanban_dto::{TimelineEntityKind, TimelineEntityRef, TimelineEventKind, TimelineScope};
use serde_json::json;

use crate::comments::SqliteCommentStore;
use crate::db::Database;
use crate::evidence::SqliteEvidenceStore;
use crate::migrations::{AllowAllMigrations, apply_through};
use crate::projects::SqliteProjectStore;
use crate::test_support::scratch_database;
use crate::timeline::TimelineFilter;
use crate::timeline::TimelineRow;

/// A database on the schema before the unification, holding one
/// Project whose history the earlier commands split across three
/// scopes: its own transitions under `1`, comments under `kan`,
/// and evidence under `kan-p1`.
fn database_with_split_project_history() -> Database {
    let (_dir, database) = scratch_database();
    apply_through(&database.connection(), 10).expect("the pre-upgrade schema applies");
    database
        .connection()
        .execute(
            "INSERT INTO projects
                 (code, name, repository, seed_workspace, default_branch,
                  herdr_session, archived, version)
             VALUES ('CORE', 'Control plane', '/repositories/kanban',
                     '/workspaces/kanban.seed', 'main', 'kanban-main', 0, 1)",
            [],
        )
        .expect("the pre-upgrade Project lands");
    for (project_id, kind, entity_kind, entity_id, detail, recorded_at) in [
        (
            "1",
            "transition",
            "project",
            "1",
            json!({ "action": "registered", "id": 1 }),
            "2026-03-01T00:00:00.000000Z",
        ),
        (
            "kan",
            "comment",
            "ticket",
            "kan-t9",
            json!({ "comment_id": 1, "text": "noted", "revision": 1 }),
            "2026-04-01T00:00:00.000000Z",
        ),
        (
            "kan-p1",
            "evidence",
            "ticket",
            "kan-t10",
            json!({ "action": "attached", "id": 1 }),
            "2026-05-01T00:00:00.000000Z",
        ),
    ] {
        database
            .connection()
            .execute(
                "INSERT INTO timeline_events
                 (scope, project_id, kind, entity_kind, entity_id, detail, recorded_at)
                 VALUES ('project', ?1, ?2, ?3, ?4, ?5, ?6)",
                rusqlite::params![
                    project_id,
                    kind,
                    entity_kind,
                    entity_id,
                    detail.to_string(),
                    recorded_at
                ],
            )
            .expect("the legacy timeline row lands");
    }
    database
        .connection()
        .execute(
            "INSERT INTO comments (project_id, entity_kind, entity_id, version)
             VALUES ('kan', 'ticket', 'kan-t9', 1)",
            [],
        )
        .expect("the legacy comment row lands");
    database
        .connection()
        .execute(
            "INSERT INTO evidence_items (project_id, entity_kind, entity_id, kind)
             VALUES ('kan-p1', 'ticket', 'kan-t10', 'managed_file')",
            [],
        )
        .expect("the legacy evidence row lands");
    database
}

/// Every timeline row as stored, with everything a repair must
/// keep: identity, scope, kind, entity, time, and facts.
fn snapshot(database: &Database) -> Vec<(i64, String, String, String, String, String)> {
    let conn = database.connection();
    let mut statement = conn
        .prepare(
            "SELECT id, project_id, kind, entity_kind, entity_id, recorded_at
             FROM timeline_events ORDER BY id",
        )
        .expect("the timeline is readable");
    statement
        .query_map([], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
            ))
        })
        .expect("the snapshot query runs")
        .collect::<Result<Vec<_>, _>>()
        .expect("the snapshot rows decode")
}

#[test]
fn the_upgrade_reunifies_split_history_without_losing_anything() {
    let mut database = database_with_split_project_history();

    database
        .migrate(&AllowAllMigrations)
        .expect("the identity migration applies");

    assert_eq!(
        snapshot(&database),
        vec![
            (
                1,
                "1".to_owned(),
                "transition".to_owned(),
                "project".to_owned(),
                "1".to_owned(),
                "2026-03-01T00:00:00.000000Z".to_owned()
            ),
            (
                2,
                "1".to_owned(),
                "comment".to_owned(),
                "ticket".to_owned(),
                "kan-t9".to_owned(),
                "2026-04-01T00:00:00.000000Z".to_owned()
            ),
            (
                3,
                "1".to_owned(),
                "evidence".to_owned(),
                "ticket".to_owned(),
                "kan-t10".to_owned(),
                "2026-05-01T00:00:00.000000Z".to_owned()
            ),
        ],
        "every row keeps its identity, time, and facts, now under the Project's identity"
    );
    let comment_project: String = database
        .connection()
        .query_row("SELECT project_id FROM comments WHERE id = 1", [], |row| {
            row.get(0)
        })
        .expect("the comment row is readable");
    assert_eq!(comment_project, "1");
    let evidence_project: String = database
        .connection()
        .query_row(
            "SELECT project_id FROM evidence_items WHERE id = 1",
            [],
            |row| row.get(0),
        )
        .expect("the evidence row is readable");
    assert_eq!(evidence_project, "1");

    // The reunified history is queryable through the typed
    // surface, complete.
    let rows: Vec<TimelineRow> = database
        .query_timeline(&TimelineFilter::of(TimelineScope::Project(1)))
        .expect("the Project timeline is readable");
    assert_eq!(rows.len(), 3, "complete Project history is queryable");
    assert_eq!(rows[0].detail, json!({ "action": "registered", "id": 1 }));
    assert_eq!(rows[1].detail["text"], json!("noted"));
}

#[test]
fn a_fresh_database_lands_every_row_under_one_scope() {
    let (dir, mut database) = scratch_database();
    database
        .migrate(&AllowAllMigrations)
        .expect("the migrations apply");

    // One registered Project driving every project-scoped writer.
    let projects = SqliteProjectStore::new(&database);
    let registration = kanban_domain::ProjectRegistration::new(
        "CORE",
        "Control plane",
        "/repositories/kanban",
        "/workspaces/kanban.seed",
        "main",
        "kanban.seed",
        Some("kanban-main"),
        None,
    )
    .expect("the fixture registration validates");
    let project = projects
        .create(&registration, &|id| {
            TimelineEnvelope::project(
                id.value(),
                TimelineEventKind::Transition,
                Some(TimelineEntityRef {
                    kind: TimelineEntityKind::Project,
                    id: id.value().to_string(),
                }),
                json!({ "action": "registered", "id": id.value() }),
            )
        })
        .expect("the registration lands");
    let comments = SqliteCommentStore::new(&database);
    comments
        .create(
            project.id().value(),
            &kanban_domain::CommentTarget::new("ticket", "kan-t10").expect("the target validates"),
            &kanban_domain::CommentText::new("one timeline").expect("the text validates"),
        )
        .expect("the comment lands");
    let evidence = SqliteEvidenceStore::new(&database, dir.path().join("attachments"));
    evidence
        .attach_managed_file(
            project.id().value(),
            "ticket",
            "kan-t10",
            "cHJvb2Y=",
            kanban_app::TimelineFacts {
                kind: TimelineEventKind::Evidence,
                facts: json!({ "action": "attached" }),
            },
        )
        .expect("the evidence lands");

    let rows = database
        .query_timeline(&TimelineFilter::of(TimelineScope::Project(
            project.id().value(),
        )))
        .expect("the Project timeline is readable");

    assert_eq!(
        rows.iter()
            .map(|row| (
                row.scope.as_str(),
                row.project_id.as_str(),
                row.kind.as_str()
            ))
            .collect::<Vec<_>>(),
        vec![
            ("project", "1", "transition"),
            ("project", "1", "comment"),
            ("project", "1", "evidence"),
        ],
        "a fresh database splits nothing"
    );
}

#[test]
fn several_projects_keep_unattributed_scopes_and_audit_them() {
    let (_dir, mut database) = scratch_database();
    apply_through(&database.connection(), 10).expect("the pre-upgrade schema applies");
    for code in ["CORE", "WAVE"] {
        database
            .connection()
            .execute(
                "INSERT INTO projects
                     (code, name, repository, seed_workspace, default_branch,
                      herdr_session, archived, version)
                 VALUES (?1, 'Pool', '/repositories/kanban',
                         '/workspaces/kanban.seed', 'main', ?2, 0, 1)",
                rusqlite::params![code, format!("{}-main", code.to_lowercase())],
            )
            .expect("the pre-upgrade Project lands");
    }
    database
        .connection()
        .execute(
            "INSERT INTO timeline_events
                 (scope, project_id, kind, entity_kind, entity_id, detail, recorded_at)
             VALUES ('project', 'kan', 'comment', 'ticket', 'kan-t9', '{}',
                     '2026-04-01T00:00:00.000000Z')",
            [],
        )
        .expect("the unattributable legacy row lands");

    database
        .migrate(&AllowAllMigrations)
        .expect("the identity migration applies");

    // Attribution would be a guess between two Projects, so the
    // row stays exactly where it was.
    assert_eq!(
        snapshot(&database),
        vec![(
            1,
            "kan".to_owned(),
            "comment".to_owned(),
            "ticket".to_owned(),
            "kan-t9".to_owned(),
            "2026-04-01T00:00:00.000000Z".to_owned(),
        )],
        "an unattributable row is preserved verbatim, never guessed at"
    );
    let audit: String = database
        .connection()
        .query_row(
            "SELECT detail FROM audit_events WHERE kind = 'timeline.scopes.unattributed'",
            [],
            |row| row.get(0),
        )
        .expect("the repair audited what it left behind");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&audit).expect("the detail is JSON"),
        json!({ "scopes": ["kan"] })
    );
}

#[test]
fn the_repair_leaves_no_update_path_behind() {
    let mut database = database_with_split_project_history();
    database
        .migrate(&AllowAllMigrations)
        .expect("the identity migration applies");
    // A row per guarded table, so each refusal has a row to refuse
    // on: triggers fire per matched row.
    for sql in [
        "INSERT INTO rulings (project_id, summary) VALUES ('1', 'Hold')",
        "INSERT INTO deferrals (project_id, finding_id, reason) VALUES ('1', 'finding-1', 'Cosmetic')",
        "INSERT INTO evidence_items (project_id, entity_kind, entity_id, kind) VALUES ('1', 'ticket', 'kan-t10', 'managed_file')",
    ] {
        database
            .connection()
            .execute(sql, [])
            .expect("the fixture row lands");
    }

    for (sql, refusal) in [
        (
            "UPDATE timeline_events SET kind = 'tampered'",
            "timeline_events is append-only",
        ),
        (
            "UPDATE rulings SET summary = 'tampered'",
            "rulings is append-only",
        ),
        (
            "UPDATE deferrals SET reason = 'tampered'",
            "deferrals is append-only",
        ),
        (
            "UPDATE evidence_items SET entity_id = 'tampered'",
            "evidence_items is append-only",
        ),
    ] {
        let outcome = database.connection().execute(sql, []);
        let error = outcome.expect_err("the schema must refuse updates again");
        assert!(
            error.to_string().contains(refusal),
            "the refusal should say `{refusal}`, got: {error}"
        );
    }
}
