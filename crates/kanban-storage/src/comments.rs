//! The SQLite implementation of the Comment storage port: rows in
//! `comments`, immutable revisions in `comment_revisions`, and the
//! timeline append landing in the same transaction as every change.

use kanban_app::{CommentStore, TimelineEnvelope};
use kanban_domain::{Comment, CommentId, CommentRevision, CommentTarget, CommentText};
use kanban_dto::{
    ApiError, CommentListItem, CommentRecord, CommentRevisionRecord, TimelineEntityKind,
    TimelineEntityRef, TimelineEventKind,
};
use rusqlite::{OptionalExtension, params};
use serde_json::json;

use crate::db::{ConnectionHandle, Database, WriteSpan};
use crate::timeline::insert_event;

/// The Comment port over the authoritative database.
pub struct SqliteCommentStore {
    conn: ConnectionHandle,
}

impl SqliteCommentStore {
    /// Share the connection the `database` owns.
    pub fn new(database: &Database) -> Self {
        Self {
            conn: database.connection_handle(),
        }
    }

    /// Lock the shared connection.
    fn lock(&self) -> parking_lot::ReentrantMutexGuard<'_, rusqlite::Connection> {
        self.conn.lock()
    }
}

impl CommentStore for SqliteCommentStore {
    fn create(
        &self,
        project_id: u64,
        target: &CommentTarget,
        text: &CommentText,
        author: &str,
        parent_comment_id: Option<u64>,
    ) -> Result<Comment, ApiError> {
        let conn = self.lock();
        let span = WriteSpan::begin(&conn).map_err(internal)?;
        span.execute(
            "INSERT INTO comments (project_id, entity_kind, entity_id, version, author, parent_comment_id)
                 VALUES (?1, ?2, ?3, 1, ?4, ?5)",
            params![
                project_id.to_string(),
                target.kind(),
                target.id(),
                author,
                parent_comment_id.map(|value| value as i64)
            ],
        )
        .map_err(internal)?;
        let id = CommentId::new(
            span.last_insert_rowid()
                .try_into()
                .map_err(|_| ApiError::internal("the Comment identity overflowed"))?,
        );
        let revision_stamp = append_revision(&span, id, 1, text)?;
        append_timeline(&span, project_id, target, id, text, 1, &revision_stamp)?;
        span.commit().map_err(internal)?;
        Ok(Comment::create(
            id,
            project_id,
            target.clone(),
            text.clone(),
        ))
    }

    fn thread_anchor(
        &self,
        id: CommentId,
    ) -> Result<Option<(CommentTarget, Option<u64>)>, ApiError> {
        let conn = self.lock();
        let row = conn.query_row(
            "SELECT entity_kind, entity_id, parent_comment_id
             FROM comments WHERE id = ?1",
            params![id.value() as i64],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<i64>>(2)?,
                ))
            },
        );
        let (entity_kind, entity_id, parent_comment_id) = match row {
            Ok(values) => values,
            Err(rusqlite::Error::QueryReturnedNoRows) => return Ok(None),
            Err(error) => return Err(ApiError::internal(&error.to_string())),
        };
        let target = decode_target(&entity_kind, &entity_id)?;
        Ok(Some((target, parent_comment_id.map(|value| value as u64))))
    }

    fn find(&self, id: CommentId) -> Result<Option<Comment>, ApiError> {
        let conn = self.lock();
        let row = conn.query_row(
            "SELECT project_id, entity_kind, entity_id, version
             FROM comments WHERE id = ?1",
            params![id.value() as i64],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?.unsigned_abs(),
                ))
            },
        );
        let (project_id, entity_kind, entity_id, version) = match row {
            Ok(values) => values,
            Err(rusqlite::Error::QueryReturnedNoRows) => return Ok(None),
            Err(error) => return Err(ApiError::internal(&error.to_string())),
        };
        let project_id = decode_project(&project_id)?;
        let target = decode_target(&entity_kind, &entity_id)?;
        let revisions = load_revisions(&conn, id)?;
        Ok(Some(Comment::restore(
            id, project_id, target, revisions, version,
        )))
    }

    fn save(&self, comment: &Comment) -> Result<(), ApiError> {
        let latest = comment
            .revisions()
            .last()
            .expect("a Comment always has at least one revision");
        let conn = self.lock();
        let span = WriteSpan::begin(&conn).map_err(internal)?;
        let changed = span
            .execute(
                "UPDATE comments SET version = ?2 WHERE id = ?1 AND version = ?3",
                params![
                    comment.id().value() as i64,
                    comment.version() as i64,
                    (comment.version() - 1) as i64,
                ],
            )
            .map_err(internal)?;
        if changed != 1 {
            return Err(ApiError::not_found(&format!("comment {}", comment.id())));
        }
        let revision_stamp = append_revision(&span, comment.id(), latest.number(), latest.text())?;
        append_timeline(
            &span,
            comment.project_id(),
            comment.target(),
            comment.id(),
            latest.text(),
            latest.number(),
            &revision_stamp,
        )?;
        span.commit().map_err(internal)?;
        Ok(())
    }

    fn revisions(
        &self,
        id: CommentId,
    ) -> Result<(CommentRecord, Vec<CommentRevisionRecord>), ApiError> {
        let comment = self
            .find(id)?
            .ok_or_else(|| ApiError::not_found(&format!("comment {id}")))?;
        let conn = self.lock();
        let mut statement = conn
            .prepare(
                "SELECT revision, text, recorded_at
                 FROM comment_revisions
                 WHERE comment_id = ?1
                 ORDER BY revision ASC",
            )
            .map_err(|error| ApiError::internal(&error.to_string()))?;
        let rows = statement
            .query_map(params![id.value() as i64], |row| {
                Ok(CommentRevisionRecord {
                    revision: row.get::<_, i64>(0)?.unsigned_abs(),
                    text: row.get(1)?,
                    recorded_at: row.get(2)?,
                })
            })
            .map_err(|error| ApiError::internal(&error.to_string()))?;
        let mut revisions = Vec::new();
        for row in rows {
            revisions.push(row.map_err(|error| ApiError::internal(&error.to_string()))?);
        }
        Ok((record_of(&comment), revisions))
    }

    fn set_resolved(&self, id: CommentId, resolved: bool) -> Result<u64, ApiError> {
        let conn = self.lock();
        // Resolving and unresolving each advance the aggregate the
        // way an edit does, so concurrent writers stay guarded; an
        // unresolve of a comment already unresolved changes nothing
        // and answers with the version it found.
        let changed = if resolved {
            conn.execute(
                "UPDATE comments
                 SET resolved_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
                     version = version + 1
                 WHERE id = ?1",
                params![id.value() as i64],
            )
            .map_err(internal)?
        } else {
            conn.execute(
                "UPDATE comments SET resolved_at = NULL, version = version + 1
                 WHERE id = ?1 AND resolved_at IS NOT NULL",
                params![id.value() as i64],
            )
            .map_err(internal)?
        };
        if changed == 0 {
            let resolved_at: Option<Option<String>> = conn
                .query_row(
                    "SELECT resolved_at FROM comments WHERE id = ?1",
                    params![id.value() as i64],
                    |row| row.get(0),
                )
                .optional()
                .map_err(internal)?;
            if resolved_at.is_none() {
                return Err(ApiError::not_found(&format!("comment {}", id.value())));
            }
            let version: i64 = conn
                .query_row(
                    "SELECT version FROM comments WHERE id = ?1",
                    params![id.value() as i64],
                    |row| row.get(0),
                )
                .map_err(internal)?;
            return Ok(version.unsigned_abs());
        }
        let version: i64 = conn
            .query_row(
                "SELECT version FROM comments WHERE id = ?1",
                params![id.value() as i64],
                |row| row.get(0),
            )
            .map_err(internal)?;
        Ok(version.unsigned_abs())
    }

    fn list_for_target(
        &self,
        target: &CommentTarget,
        limit: u32,
        cursor: Option<u64>,
    ) -> Result<(Vec<CommentListItem>, Option<String>), ApiError> {
        let conn = self.lock();
        let after = cursor.unwrap_or(0) as i64;
        let ticket_id = (target.kind() == "ticket")
            .then(|| target.id().parse::<i64>().ok())
            .flatten();
        let project_code = ticket_id
            .and_then(|id| {
                conn.query_row(
                    "SELECT p.code FROM tickets AS t
                     JOIN projects AS p ON p.id = t.project_id
                     WHERE t.id = ?1",
                    [id],
                    |row| row.get::<_, String>(0),
                )
                .optional()
                .ok()
                .flatten()
            })
            .unwrap_or_default();
        let ticket_stream = i64::from(ticket_id.is_some());
        let mut statement = conn
            .prepare(
                "SELECT c.id, c.version, c.created_at,
                        (SELECT text FROM comment_revisions
                         WHERE comment_id = c.id ORDER BY revision DESC LIMIT 1),
                        (SELECT revision FROM comment_revisions
                         WHERE comment_id = c.id ORDER BY revision DESC LIMIT 1),
                        (SELECT recorded_at FROM comment_revisions
                         WHERE comment_id = c.id ORDER BY revision DESC LIMIT 1),
                        COALESCE(c.author, ''),
                        c.parent_comment_id,
                        c.resolved_at
                 FROM comments AS c
                 WHERE c.entity_kind = ?1 AND c.id > ?3 AND (
                     c.entity_id = ?2
                     OR (
                       ?4 = 1 AND (
                         (
                           (c.entity_id GLOB '+[0-9]*' OR c.entity_id GLOB '[0-9]*')
                           AND CAST(c.entity_id AS INTEGER) = CAST(?2 AS INTEGER)
                         )
                         OR (
                           ?5 != ''
                           AND UPPER(substr(c.entity_id, 1, length(?5) + 1)) = UPPER(?5 || '-')
                           AND substr(c.entity_id, length(?5) + 2) GLOB '[0-9]*'
                           AND CAST(substr(c.entity_id, length(?5) + 2) AS INTEGER) = CAST(?2 AS INTEGER)
                         )
                       )
                     )
                 )
                 ORDER BY c.id ASC",
            )
            .map_err(|error| ApiError::internal(&error.to_string()))?;
        let rows = statement
            .query_map(
                rusqlite::params![
                    target.kind(),
                    target.id(),
                    after,
                    ticket_stream,
                    project_code
                ],
                |row| {
                    let id: i64 = row.get(0)?;
                    let version: i64 = row.get(1)?;
                    let created_at: String = row.get(2)?;
                    let body: String = row.get(3)?;
                    let latest_revision: i64 = row.get(4)?;
                    let latest_at: String = row.get(5)?;
                    let author: Option<String> = row.get(6)?;
                    let parent_comment_id: Option<i64> = row.get(7)?;
                    let resolved_at: Option<String> = row.get(8)?;
                    Ok((
                        id,
                        version,
                        created_at,
                        body,
                        latest_revision,
                        latest_at,
                        author,
                        parent_comment_id,
                        resolved_at,
                    ))
                },
            )
            .map_err(|error| ApiError::internal(&error.to_string()))?;
        let mut items = Vec::new();
        for row in rows {
            let (
                id,
                version,
                created_at,
                body,
                latest_revision,
                latest_at,
                author,
                parent_comment_id,
                resolved_at,
            ) = row.map_err(|error| ApiError::internal(&error.to_string()))?;
            // Edit metadata follows the stored body revisions: a
            // resolution change advances the aggregate without
            // touching the body, so only a revision past the
            // original reports an edit.
            let edited_at = (latest_revision > 1).then_some(latest_at);
            items.push(CommentListItem {
                id: id.unsigned_abs(),
                author: author.unwrap_or_default(),
                body,
                created_at,
                edited_at,
                parent_comment_id: parent_comment_id.map(|value| value as u64),
                resolved_at,
                version: version as u64,
            });
            if items.len() > limit as usize {
                break;
            }
        }
        let next_cursor = if items.len() > limit as usize {
            Some(items[(limit as usize).saturating_sub(1)].id.to_string())
        } else {
            None
        };
        items.truncate(limit as usize);
        Ok((items, next_cursor))
    }
}

/// Report a SQLite failure the caller cannot act on.
fn internal(error: rusqlite::Error) -> ApiError {
    ApiError::internal(&error.to_string())
}

/// The numeric Project identity a stored row carries. Anything else is
/// a legacy scope migration 0017 missed, which is corruption the
/// caller must hear about rather than quietly reinterpret.
fn decode_project(project_id: &str) -> Result<u64, ApiError> {
    project_id.parse().map_err(|_| {
        ApiError::internal(&format!(
            "a stored comment names the non-numeric Project scope `{project_id}`"
        ))
    })
}

fn decode_target(entity_kind: &str, entity_id: &str) -> Result<CommentTarget, ApiError> {
    CommentTarget::new(entity_kind, entity_id)
        .map_err(|error| ApiError::internal(&error.to_string()))
}

fn load_revisions(
    conn: &rusqlite::Connection,
    id: CommentId,
) -> Result<Vec<CommentRevision>, ApiError> {
    let mut statement = conn
        .prepare(
            "SELECT revision, text
             FROM comment_revisions
             WHERE comment_id = ?1
             ORDER BY revision ASC",
        )
        .map_err(|error| ApiError::internal(&error.to_string()))?;
    let rows = statement
        .query_map(params![id.value() as i64], |row| {
            let revision = row.get::<_, i64>(0)?.unsigned_abs();
            let text: String = row.get(1)?;
            // Stored operator bodies keep their whitespace verbatim.
            let text = CommentText::new_stored(&text)
                .map_err(|_| rusqlite::Error::ToSqlConversionFailure(Box::new(CorruptText)))?;
            Ok(CommentRevision::restore(revision, text))
        })
        .map_err(|error| ApiError::internal(&error.to_string()))?;
    let mut revisions = Vec::new();
    for row in rows {
        revisions.push(row.map_err(|error| ApiError::internal(&error.to_string()))?);
    }
    Ok(revisions)
}

/// Stored text failed domain validation.
#[derive(Debug)]
struct CorruptText;

impl std::fmt::Display for CorruptText {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "a stored comment revision failed validation")
    }
}

impl std::error::Error for CorruptText {}

fn append_revision(
    conn: &rusqlite::Connection,
    id: CommentId,
    revision: u64,
    text: &CommentText,
) -> Result<String, ApiError> {
    conn.execute(
        "INSERT INTO comment_revisions (comment_id, revision, text)
             VALUES (?1, ?2, ?3)",
        params![id.value() as i64, revision as i64, text.as_str()],
    )
    .map_err(internal)?;
    let recorded_at: String = conn
        .query_row(
            "SELECT recorded_at FROM comment_revisions
             WHERE comment_id = ?1 AND revision = ?2",
            params![id.value() as i64, revision as i64],
            |row| row.get(0),
        )
        .map_err(internal)?;
    Ok(recorded_at)
}

fn append_timeline(
    conn: &rusqlite::Connection,
    project_id: u64,
    target: &CommentTarget,
    id: CommentId,
    text: &CommentText,
    revision: u64,
    recorded_at: &str,
) -> Result<(), ApiError> {
    let envelope = TimelineEnvelope::project(
        project_id,
        TimelineEventKind::Comment,
        Some(TimelineEntityRef {
            // The target passed the vocabulary check on the way in;
            // anything else is corruption.
            kind: TimelineEntityKind::parse(target.kind())
                .ok_or_else(|| ApiError::internal("a stored Comment target is corrupt"))?,
            id: target.id().to_owned(),
        }),
        json!({
            "comment_id": id.value(),
            "text": text.as_str(),
            "revision": revision,
            "recorded_at": recorded_at,
        }),
    );
    insert_event(conn, &envelope).map_err(|error| ApiError::internal(&error.to_string()))
}

fn record_of(comment: &Comment) -> CommentRecord {
    CommentRecord {
        id: comment.id().value(),
        project_id: comment.project_id(),
        target: TimelineEntityRef {
            // The target passed the vocabulary check on the way
            // in; anything else is corruption.
            kind: TimelineEntityKind::parse(comment.target().kind())
                .expect("a stored Comment target names a known entity kind"),
            id: comment.target().id().to_owned(),
        },
        text: comment.current_text().as_str().to_owned(),
        version: comment.version(),
    }
}

#[cfg(test)]
mod revision_store {
    use kanban_app::CommentStore;
    use kanban_domain::{CommentId, CommentTarget, CommentText};
    use kanban_dto::ErrorCode;
    use serde_json::json;

    use super::SqliteCommentStore;
    use crate::db::Database;
    use crate::migrations::AllowAllMigrations;
    use crate::test_support::scratch_database;

    fn store() -> (tempfile::TempDir, Database, SqliteCommentStore) {
        let (dir, mut database) = scratch_database();
        database
            .migrate(&AllowAllMigrations)
            .expect("the migrations apply");
        let store = SqliteCommentStore::new(&database);
        (dir, database, store)
    }

    fn target() -> CommentTarget {
        CommentTarget::new("ticket", "kan-t11").expect("the target validates")
    }

    fn text(value: &str) -> CommentText {
        CommentText::new(value).expect("the text validates")
    }

    fn revision_rows(database: &Database) -> Vec<(u64, u64, String)> {
        let conn = database.connection();
        let mut statement = conn
            .prepare("SELECT comment_id, revision, text FROM comment_revisions ORDER BY id")
            .expect("the revisions table is readable");
        statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?.unsigned_abs(),
                    row.get::<_, i64>(1)?.unsigned_abs(),
                    row.get::<_, String>(2)?,
                ))
            })
            .expect("the revision query runs")
            .collect::<Result<Vec<_>, _>>()
            .expect("the rows decode")
    }

    fn timeline_rows(database: &Database) -> Vec<(String, serde_json::Value)> {
        let conn = database.connection();
        let mut statement = conn
            .prepare("SELECT kind, detail FROM timeline_events ORDER BY id")
            .expect("the timeline is readable");
        statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    serde_json::from_str(&row.get::<_, String>(1)?)
                        .expect("the stored detail is JSON"),
                ))
            })
            .expect("the timeline query runs")
            .collect::<Result<Vec<_>, _>>()
            .expect("the timeline rows decode")
    }

    #[test]
    fn creating_lands_the_comment_revision_and_timeline_event() {
        let (_dir, database, store) = store();

        let comment = store
            .create(1, &target(), &text("first thought"), "test", None)
            .expect("the create lands");

        assert_eq!(comment.id().value(), 1);
        assert_eq!(comment.current_text().as_str(), "first thought");
        assert_eq!(
            revision_rows(&database),
            vec![(1, 1, "first thought".to_owned())]
        );
        let (kind, detail) = timeline_rows(&database)
            .last()
            .cloned()
            .expect("the timeline event landed");
        assert_eq!(kind, "comment");
        assert_eq!(detail["comment_id"], json!(1));
        assert_eq!(detail["text"], json!("first thought"));
        assert_eq!(detail["revision"], json!(1));
    }

    #[test]
    fn editing_appends_a_revision_and_updates_current_text() {
        let (_dir, database, store) = store();
        let mut comment = store
            .create(1, &target(), &text("first thought"), "test", None)
            .expect("the create lands");
        comment
            .edit(text("corrected thought"))
            .expect("the domain edit applies");

        store.save(&comment).expect("the save lands");

        let reloaded = store
            .find(CommentId::new(1))
            .expect("the find serves")
            .expect("the Comment exists");
        assert_eq!(reloaded.current_text().as_str(), "corrected thought");
        assert_eq!(reloaded.version(), 2);
        assert_eq!(
            revision_rows(&database),
            vec![
                (1, 1, "first thought".to_owned()),
                (1, 2, "corrected thought".to_owned()),
            ]
        );
    }

    #[test]
    fn revisions_query_returns_history_in_order() {
        let (_dir, _database, store) = store();
        let mut comment = store
            .create(1, &target(), &text("first thought"), "test", None)
            .expect("the create lands");
        comment
            .edit(text("second thought"))
            .expect("the domain edit applies");
        store.save(&comment).expect("the save lands");

        let (record, revisions) = store
            .revisions(CommentId::new(1))
            .expect("the revisions query serves");

        assert_eq!(record.text, "second thought");
        assert_eq!(record.version, 2);
        assert_eq!(revisions.len(), 2);
        assert_eq!(revisions[0].revision, 1);
        assert_eq!(revisions[0].text, "first thought");
        assert_eq!(revisions[1].revision, 2);
        assert_eq!(revisions[1].text, "second thought");
    }

    #[test]
    fn updating_comment_revisions_fails() {
        let (_dir, database, store) = store();
        store
            .create(1, &target(), &text("first thought"), "test", None)
            .expect("the create lands");

        let outcome = database
            .connection()
            .execute("UPDATE comment_revisions SET text = 'tampered'", []);

        let error = outcome.expect_err("the schema must refuse updates");
        assert!(
            error.to_string().contains("append-only"),
            "the refusal should say append-only, got: {error}"
        );
    }

    #[test]
    fn deleting_comment_revisions_fails() {
        let (_dir, database, store) = store();
        store
            .create(1, &target(), &text("first thought"), "test", None)
            .expect("the create lands");

        let outcome = database
            .connection()
            .execute("DELETE FROM comment_revisions", []);

        let error = outcome.expect_err("the schema must refuse deletes");
        assert!(
            error.to_string().contains("append-only"),
            "the refusal should say append-only, got: {error}"
        );
    }

    #[test]
    fn saving_an_unknown_comment_is_not_found() {
        let (_dir, _database, store) = store();
        let ghost = kanban_domain::Comment::create(CommentId::new(9), 1, target(), text("ghost"));

        let error = store
            .save(&ghost)
            .expect_err("the unknown Comment is refused");

        assert_eq!(error.code, ErrorCode::NotFound);
    }

    fn project_target(id: &str) -> CommentTarget {
        CommentTarget::new("project", id).expect("the target validates")
    }

    #[test]
    fn legacy_ticket_addresses_migrate_to_the_canonical_id() {
        let (dir, mut database) = scratch_database();
        crate::migrations::apply_through(&database.connection(), 61)
            .expect("the earlier migrations apply");
        let conn = database.connection();
        conn.execute(
            "INSERT INTO projects
             (code, name, repository, seed_workspace, default_branch,
              herdr_session, herdr_workspace, archived, version)
             VALUES ('CORE', 'Control plane', '/repo', '/seed', 'main',
                     'kanban-main', 'kanban.seed', 0, 1)",
            [],
        )
        .expect("the fixture project lands");
        conn.execute(
            "INSERT INTO tickets (
                 id, project_id, number, kind, priority, state, title, criteria,
                 subtype, mode, completion, due, version, created_at
             ) VALUES
                 (1, 1, 1, 'task', 'normal', 'draft', 'One', '[]',
                  'operational', 'human', '[\"Shipped\"]', NULL, 1, '2026-01-01T00:00:00.000Z'),
                 (2, 1, 2, 'task', 'normal', 'draft', 'Two', '[]',
                  'operational', 'human', '[\"Shipped\"]', NULL, 1, '2026-01-01T00:00:00.000Z'),
                 (3, 1, 3, 'task', 'normal', 'draft', 'Three', '[]',
                  'operational', 'human', '[\"Shipped\"]', NULL, 1, '2026-01-01T00:00:00.000Z')",
            [],
        )
        .expect("the fixture tickets land");
        for (id, stored) in [
            (1, "1"),
            (2, "Core-01"),
            (3, "CORE-1"),
            (4, "+1"),
            (5, "2"),
            (6, "NOPE-9"),
        ] {
            conn.execute(
                "INSERT INTO comments (id, project_id, entity_kind, entity_id, version, author)
                 VALUES (?1, '1', 'ticket', ?2, 1, 'tester')",
                rusqlite::params![id, stored],
            )
            .expect("the legacy row lands");
        }
        drop(conn);
        database
            .migrate(&AllowAllMigrations)
            .expect("the migrations apply");
        let conn = database.connection();
        let mut statement = conn
            .prepare("SELECT id, entity_id FROM comments ORDER BY id")
            .expect("the rows read");
        let rows: Vec<(i64, String)> = statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .expect("the rows read")
            .collect::<Result<Vec<_>, _>>()
            .expect("the rows read");
        assert_eq!(
            rows,
            vec![
                (1, "1".to_owned()),
                (2, "1".to_owned()),
                (3, "1".to_owned()),
                (4, "1".to_owned()),
                (5, "2".to_owned()),
                (6, "NOPE-9".to_owned()),
            ],
            "every resolvable spelling becomes the numeric id; an unresolvable one stays"
        );
        drop(statement);
        drop(conn);
        let _ = dir;
    }

    #[test]
    fn non_ticket_streams_list_by_exact_id() {
        let (_dir, _database, store) = store();
        store
            .create(
                1,
                &project_target("Alpha"),
                &text("on Alpha"),
                "tester",
                None,
            )
            .expect("the Alpha comment lands");
        store
            .create(
                1,
                &project_target("alpha"),
                &text("on alpha"),
                "tester",
                None,
            )
            .expect("the alpha comment lands");

        let (upper, _) = store
            .list_for_target(&project_target("Alpha"), 10, None)
            .expect("the Alpha list serves");
        assert_eq!(upper.len(), 1, "only the exact-id row lists");
        assert_eq!(upper[0].body, "on Alpha");

        let (lower, _) = store
            .list_for_target(&project_target("alpha"), 10, None)
            .expect("the alpha list serves");
        assert_eq!(lower.len(), 1, "only the exact-id row lists");
        assert_eq!(lower[0].body, "on alpha");
    }

    #[test]
    fn revisions_keep_the_stored_text_verbatim() {
        let (_dir, _database, store) = store();
        let body = CommentText::new_stored("Report\n").expect("stored text validates");
        store
            .create(1, &target(), &body, "tester", None)
            .expect("the create lands");

        let (record, revisions) = store
            .revisions(CommentId::new(1))
            .expect("the revisions serve");

        assert_eq!(record.text, "Report\n");
        assert_eq!(revisions[0].text, "Report\n");
    }

    #[test]
    fn list_for_target_filters_by_id_and_pages_past_the_cursor() {
        let (_dir, _database, store) = store();
        for (row, ticket) in [(1, "1"), (2, "1"), (3, "2"), (4, "1")] {
            store
                .create(
                    1,
                    &CommentTarget::new("ticket", ticket).expect("the target validates"),
                    &text(format!("row {row}").as_str()),
                    "tester",
                    None,
                )
                .expect("the row lands");
        }

        let first_issue = CommentTarget::new("ticket", "1").expect("the target validates");
        let mut listed = Vec::new();
        let mut cursor = None;
        loop {
            let (page, next) = store
                .list_for_target(&first_issue, 2, cursor)
                .expect("the page serves");
            listed.extend(page.iter().map(|row| row.body.clone()));
            cursor = next.and_then(|value| value.parse::<u64>().ok());
            if cursor.is_none() {
                break;
            }
        }

        assert_eq!(
            listed,
            vec!["row 1", "row 2", "row 4"],
            "only the target's own rows list, oldest first, without duplicates"
        );
    }

    #[test]
    fn resolving_and_unresolving_each_bump_the_version() {
        let (_dir, _database, store) = store();
        store
            .create(1, &target(), &text("first thought"), "tester", None)
            .expect("the create lands");

        store
            .set_resolved(CommentId::new(1), true)
            .expect("the resolve lands");
        let (items, _) = store
            .list_for_target(&target(), 10, None)
            .expect("the list serves");
        assert_eq!(items[0].version, 2);
        assert!(items[0].resolved_at.is_some());

        store
            .set_resolved(CommentId::new(1), false)
            .expect("the unresolve lands");
        let (items, _) = store
            .list_for_target(&target(), 10, None)
            .expect("the list serves");
        assert_eq!(items[0].version, 3);
        assert!(items[0].resolved_at.is_none());
    }
    #[test]
    fn unresolving_an_unresolved_comment_changes_nothing() {
        let (_dir, _database, store) = store();
        store
            .create(1, &target(), &text("first thought"), "tester", None)
            .expect("the create lands");

        let version = store
            .set_resolved(CommentId::new(1), false)
            .expect("the no-op unresolve serves");
        assert_eq!(version, 1, "an unresolved comment keeps its version");
        let (items, _) = store
            .list_for_target(&target(), 10, None)
            .expect("the list serves");
        assert_eq!(items[0].version, 1);
        assert!(items[0].resolved_at.is_none());
    }

    #[test]
    fn resolution_changes_never_report_a_body_edit() {
        let (_dir, _database, store) = store();
        store
            .create(1, &target(), &text("first thought"), "tester", None)
            .expect("the create lands");
        let row = |store: &SqliteCommentStore| {
            let (items, _) = store
                .list_for_target(&target(), 10, None)
                .expect("the list serves");
            items[0].clone()
        };
        let revision_count = |store: &SqliteCommentStore| {
            store
                .revisions(CommentId::new(1))
                .expect("the revisions serve")
                .1
                .len()
        };

        let fresh = row(&store);
        assert_eq!(fresh.version, 1);
        assert!(
            fresh.edited_at.is_none(),
            "the original revision is not an edit"
        );

        for (expected_version, resolved) in [(2, true), (3, false), (4, true)] {
            store
                .set_resolved(CommentId::new(1), resolved)
                .expect("the resolution change lands");
            let changed = row(&store);
            assert_eq!(
                changed.version, expected_version,
                "each resolution change advances the aggregate"
            );
            assert!(
                changed.edited_at.is_none(),
                "a resolution change is not a body edit"
            );
            assert_eq!(
                revision_count(&store),
                1,
                "no body revision may appear without an edit"
            );
        }

        let mut comment = store
            .find(CommentId::new(1))
            .expect("the find serves")
            .expect("the Comment exists");
        comment
            .edit(text("corrected thought"))
            .expect("the domain edit applies");
        store.save(&comment).expect("the save lands");

        let edited = row(&store);
        let (_, revisions) = store
            .revisions(CommentId::new(1))
            .expect("the revisions serve");
        assert_eq!(revisions.len(), 2);
        let edit_stamp = edited.edited_at.clone().expect("the edit reports a stamp");
        assert_eq!(
            edit_stamp,
            revisions[revisions.len() - 1].recorded_at,
            "an actual edit reports the latest revision's stamp"
        );

        store
            .set_resolved(CommentId::new(1), false)
            .expect("the unresolve after the edit lands");
        store
            .set_resolved(CommentId::new(1), true)
            .expect("the resolve after the edit lands");
        let after = row(&store);
        assert_eq!(after.version, 7);
        assert_eq!(
            after.edited_at,
            Some(edit_stamp),
            "resolution leaves the edit stamp alone"
        );
        assert_eq!(
            revision_count(&store),
            2,
            "resolution adds no body revision"
        );
    }
}
