//! The SQLite implementation of the Comment storage port: rows in
//! `comments`, immutable revisions in `comment_revisions`, and the
//! timeline append landing in the same transaction as every change.

use kanban_app::CommentStore;
use kanban_domain::{Comment, CommentId, CommentRevision, CommentTarget, CommentText};
use kanban_dto::{
    ApiError, CommentRecord, CommentRevisionRecord, TimelineEntityKind, TimelineEntityRef,
};
use rusqlite::params;
use serde_json::json;

use crate::db::{ConnectionHandle, Database, WriteSpan};
use crate::timeline::{TimelineAppend as StorageTimelineAppend, insert_event};

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
        project_id: &str,
        target: &CommentTarget,
        text: &CommentText,
    ) -> Result<Comment, ApiError> {
        let conn = self.lock();
        let span = WriteSpan::begin(&conn).map_err(internal)?;
        span.execute(
            "INSERT INTO comments (project_id, entity_kind, entity_id, version)
                 VALUES (?1, ?2, ?3, 1)",
            params![project_id, target.kind(), target.id()],
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
}

/// Report a SQLite failure the caller cannot act on.
fn internal(error: rusqlite::Error) -> ApiError {
    ApiError::internal(&error.to_string())
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
            let text = CommentText::new(&text)
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
    project_id: &str,
    target: &CommentTarget,
    id: CommentId,
    text: &CommentText,
    revision: u64,
    recorded_at: &str,
) -> Result<(), ApiError> {
    insert_event(
        conn,
        &StorageTimelineAppend {
            project_id: project_id.to_owned(),
            kind: "comment".to_owned(),
            entity_kind: Some(target.kind().to_owned()),
            entity_id: Some(target.id().to_owned()),
            detail: json!({
                "comment_id": id.value(),
                "text": text.as_str(),
                "revision": revision,
                "recorded_at": recorded_at,
            }),
        },
    )
    .map_err(|error| ApiError::internal(&error.to_string()))
}

fn record_of(comment: &Comment) -> CommentRecord {
    CommentRecord {
        id: comment.id().value(),
        project_id: comment.project_id().to_owned(),
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
            .create("kan", &target(), &text("first thought"))
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
            .create("kan", &target(), &text("first thought"))
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
            .create("kan", &target(), &text("first thought"))
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
            .create("kan", &target(), &text("first thought"))
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
            .create("kan", &target(), &text("first thought"))
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
        let ghost =
            kanban_domain::Comment::create(CommentId::new(9), "kan", target(), text("ghost"));

        let error = store
            .save(&ghost)
            .expect_err("the unknown Comment is refused");

        assert_eq!(error.code, ErrorCode::NotFound);
    }
}
