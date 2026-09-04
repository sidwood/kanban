//! The SQLite implementation of the evidence storage port: metadata
//! rows, hash-backed attachment bytes, and timeline appends in the
//! same transaction as every change.

use std::fs;
use std::path::{Path, PathBuf};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use kanban_app::{EvidenceFilter, EvidenceStore, TimelineAppend};
use kanban_domain::{
    CommitIdentity, ContentHash, EvidenceId, EvidenceItem, EvidenceKind, EvidenceShape,
    RelativePath,
};
use kanban_dto::ApiError;
use rusqlite::params;
use sha2::{Digest, Sha256};

use crate::db::{ConnectionHandle, Database};
use crate::timeline::{TimelineAppend as StorageTimelineAppend, insert_event};

/// The evidence port over the authoritative database and attachment
/// directory.
pub struct SqliteEvidenceStore {
    conn: ConnectionHandle,
    attachments_dir: PathBuf,
}

impl SqliteEvidenceStore {
    /// Share the connection the `database` owns and write bytes under
    /// `attachments_dir`.
    pub fn new(database: &Database, attachments_dir: PathBuf) -> Self {
        Self {
            conn: database.connection_handle(),
            attachments_dir,
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, rusqlite::Connection> {
        self.conn
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

impl EvidenceStore for SqliteEvidenceStore {
    fn attach_managed_file(
        &self,
        project_id: &str,
        entity_kind: &str,
        entity_id: &str,
        content_base64: &str,
        append: TimelineAppend,
    ) -> Result<EvidenceItem, ApiError> {
        let content = STANDARD
            .decode(content_base64)
            .map_err(|error| ApiError::invalid_request(&error.to_string()))?;
        let hash = content_hash(&content);
        let content_hash = ContentHash::new(&hash)
            .map_err(|error| ApiError::invalid_request(&error.to_string()))?;
        write_attachment(&self.attachments_dir, &content_hash, &content)?;
        self.insert_row(
            EvidenceShape {
                project_id: project_id.to_owned(),
                entity_kind: entity_kind.to_owned(),
                entity_id: entity_id.to_owned(),
                kind: EvidenceKind::ManagedFile,
                content_hash: Some(content_hash),
                relative_path: None,
                commit_identity: None,
            },
            append,
        )
    }

    fn attach_repository(
        &self,
        project_id: &str,
        entity_kind: &str,
        entity_id: &str,
        relative_path: &RelativePath,
        commit_identity: &CommitIdentity,
        append: TimelineAppend,
    ) -> Result<EvidenceItem, ApiError> {
        self.insert_row(
            EvidenceShape {
                project_id: project_id.to_owned(),
                entity_kind: entity_kind.to_owned(),
                entity_id: entity_id.to_owned(),
                kind: EvidenceKind::Repository,
                content_hash: None,
                relative_path: Some(relative_path.clone()),
                commit_identity: Some(commit_identity.clone()),
            },
            append,
        )
    }

    fn list(
        &self,
        filter: &EvidenceFilter,
        append: TimelineAppend,
    ) -> Result<Vec<EvidenceItem>, ApiError> {
        let mut conn = self.lock();
        let transaction = conn
            .transaction()
            .map_err(|error| ApiError::internal(&error.to_string()))?;
        let items = query_rows(&transaction, filter)?;
        append_timeline(
            &transaction,
            &filter.project_id,
            entity_kind_wire(&append),
            entity_id_wire(&append),
            &append,
        )?;
        transaction
            .commit()
            .map_err(|error| ApiError::internal(&error.to_string()))?;
        Ok(items)
    }
}

impl SqliteEvidenceStore {
    /// Read managed attachment bytes, verifying the stored hash.
    pub fn read_managed_file(&self, hash: &ContentHash) -> Result<Vec<u8>, ApiError> {
        let path = attachment_path(&self.attachments_dir, hash);
        let bytes = fs::read(&path).map_err(|error| ApiError::not_found(&error.to_string()))?;
        let actual = content_hash(&bytes);
        if actual != hash.as_str() {
            return Err(ApiError::internal("managed attachment hash mismatch"));
        }
        Ok(bytes)
    }

    fn insert_row(
        &self,
        shape: EvidenceShape,
        append: TimelineAppend,
    ) -> Result<EvidenceItem, ApiError> {
        let mut conn = self.lock();
        let transaction = conn
            .transaction()
            .map_err(|error| ApiError::internal(&error.to_string()))?;
        let kind_wire = evidence_kind_wire(shape.kind);
        transaction
            .execute(
                "INSERT INTO evidence_items (
                     project_id, entity_kind, entity_id, kind,
                     content_hash, relative_path, commit_identity
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    shape.project_id,
                    shape.entity_kind,
                    shape.entity_id,
                    kind_wire,
                    shape.content_hash.as_ref().map(ContentHash::as_str),
                    shape.relative_path.as_ref().map(RelativePath::as_str),
                    shape.commit_identity.as_ref().map(CommitIdentity::as_str),
                ],
            )
            .map_err(|error| ApiError::internal(&error.to_string()))?;
        let id = EvidenceId::new(
            transaction
                .last_insert_rowid()
                .try_into()
                .map_err(|_| ApiError::internal("the evidence identity overflowed"))?,
        );
        let item = EvidenceItem::restore(id, shape)
            .map_err(|error| ApiError::internal(&error.to_string()))?;
        let mut timeline_facts = append.facts.clone();
        timeline_facts
            .as_object_mut()
            .ok_or_else(|| ApiError::internal("timeline facts must be a JSON object"))?
            .insert("id".to_owned(), serde_json::Value::from(id.value()));
        let timeline_append = TimelineAppend {
            kind: append.kind,
            facts: timeline_facts,
        };
        append_timeline(
            &transaction,
            item.project_id(),
            item.entity_kind(),
            item.entity_id(),
            &timeline_append,
        )?;
        transaction
            .commit()
            .map_err(|error| ApiError::internal(&error.to_string()))?;
        Ok(item)
    }
}

/// The lowercase SHA-256 digest of `content`.
pub fn content_hash(content: &[u8]) -> String {
    let digest = Sha256::digest(content);
    format!("{:x}", digest)
}

/// Write managed attachment bytes before the metadata row lands.
fn write_attachment(
    attachments_dir: &Path,
    hash: &ContentHash,
    content: &[u8],
) -> Result<(), ApiError> {
    fs::create_dir_all(attachments_dir).map_err(|error| ApiError::internal(&error.to_string()))?;
    let path = attachment_path(attachments_dir, hash);
    if path.exists() {
        let existing = fs::read(&path).map_err(|error| ApiError::internal(&error.to_string()))?;
        if content_hash(&existing) == hash.as_str() {
            return Ok(());
        }
        return Err(ApiError::internal("attachment path collision"));
    }
    fs::write(&path, content).map_err(|error| ApiError::internal(&error.to_string()))?;
    Ok(())
}

fn attachment_path(attachments_dir: &Path, hash: &ContentHash) -> PathBuf {
    attachments_dir.join(hash.as_str())
}

fn query_rows(
    conn: &rusqlite::Connection,
    filter: &EvidenceFilter,
) -> Result<Vec<EvidenceItem>, ApiError> {
    let mut sql = String::from(
        "SELECT id, project_id, entity_kind, entity_id, kind,
                content_hash, relative_path, commit_identity
         FROM evidence_items
         WHERE project_id = ?1",
    );
    let mut bindings: Vec<String> = vec![filter.project_id.clone()];

    if let Some(entity_kind) = &filter.entity_kind {
        sql.push_str(" AND entity_kind = ?");
        bindings.push(entity_kind.clone());
    }
    if let Some(entity_id) = &filter.entity_id {
        sql.push_str(" AND entity_id = ?");
        bindings.push(entity_id.clone());
    }
    sql.push_str(" ORDER BY id");

    let mut statement = conn
        .prepare(&sql)
        .map_err(|error| ApiError::internal(&error.to_string()))?;
    let rows = statement
        .query_map(rusqlite::params_from_iter(bindings.iter()), decode_row)
        .map_err(|error| ApiError::internal(&error.to_string()))?;
    let mut items = Vec::new();
    for row in rows {
        items.push(row.map_err(|error| ApiError::internal(&error.to_string()))?);
    }
    Ok(items)
}

fn decode_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<EvidenceItem> {
    let id = row.get::<_, i64>(0)?.unsigned_abs();
    let project_id: String = row.get(1)?;
    let entity_kind: String = row.get(2)?;
    let entity_id: String = row.get(3)?;
    let kind_wire: String = row.get(4)?;
    let content_hash: Option<String> = row.get(5)?;
    let relative_path: Option<String> = row.get(6)?;
    let commit_identity: Option<String> = row.get(7)?;
    let kind = parse_evidence_kind(&kind_wire)?;
    let content_hash = content_hash
        .map(|value| ContentHash::new(&value))
        .transpose()
        .map_err(|_| rusqlite::Error::ToSqlConversionFailure(Box::new(CorruptEvidence)))?;
    let relative_path = relative_path
        .map(|value| RelativePath::new(&value))
        .transpose()
        .map_err(|_| rusqlite::Error::ToSqlConversionFailure(Box::new(CorruptEvidence)))?;
    let commit_identity = commit_identity
        .map(|value| CommitIdentity::new(&value))
        .transpose()
        .map_err(|_| rusqlite::Error::ToSqlConversionFailure(Box::new(CorruptEvidence)))?;
    EvidenceItem::restore(
        EvidenceId::new(id),
        EvidenceShape {
            project_id,
            entity_kind,
            entity_id,
            kind,
            content_hash,
            relative_path,
            commit_identity,
        },
    )
    .map_err(|_| rusqlite::Error::ToSqlConversionFailure(Box::new(CorruptEvidence)))
}

fn parse_evidence_kind(raw: &str) -> rusqlite::Result<EvidenceKind> {
    match raw {
        "managed_file" => Ok(EvidenceKind::ManagedFile),
        "repository" => Ok(EvidenceKind::Repository),
        _ => Err(rusqlite::Error::ToSqlConversionFailure(Box::new(
            CorruptEvidence,
        ))),
    }
}

fn evidence_kind_wire(kind: EvidenceKind) -> &'static str {
    match kind {
        EvidenceKind::ManagedFile => "managed_file",
        EvidenceKind::Repository => "repository",
    }
}

fn append_timeline(
    transaction: &rusqlite::Transaction<'_>,
    project_id: &str,
    entity_kind: &str,
    entity_id: &str,
    append: &TimelineAppend,
) -> Result<(), ApiError> {
    let mut detail = append.facts.clone();
    detail
        .as_object_mut()
        .ok_or_else(|| ApiError::internal("timeline facts must be a JSON object"))?
        .insert(
            "entity_kind".to_owned(),
            serde_json::Value::from(entity_kind),
        );
    detail
        .as_object_mut()
        .expect("the facts remain an object")
        .insert("entity_id".to_owned(), serde_json::Value::from(entity_id));
    insert_event(
        transaction,
        &StorageTimelineAppend {
            project_id: project_id.to_owned(),
            kind: append.kind.to_owned(),
            entity_kind: Some("evidence".to_owned()),
            entity_id: detail
                .get("id")
                .and_then(|value| value.as_u64())
                .map(|id| id.to_string()),
            detail,
        },
    )
    .map_err(|error| ApiError::internal(&error.to_string()))
}

fn entity_kind_wire(append: &TimelineAppend) -> &str {
    append
        .facts
        .get("entity_kind")
        .and_then(|value| value.as_str())
        .unwrap_or("")
}

fn entity_id_wire(append: &TimelineAppend) -> &str {
    append
        .facts
        .get("entity_id")
        .and_then(|value| value.as_str())
        .unwrap_or("")
}

#[derive(Debug)]
struct CorruptEvidence;

impl std::fmt::Display for CorruptEvidence {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "a stored evidence row failed validation")
    }
}

impl std::error::Error for CorruptEvidence {}

#[cfg(test)]
mod evidence_storage {
    use base64::{Engine as _, engine::general_purpose::STANDARD};
    use kanban_app::{EvidenceFilter, EvidenceStore, TimelineAppend};
    use kanban_domain::{CommitIdentity, EvidenceKind, RelativePath};
    use serde_json::json;

    use super::{SqliteEvidenceStore, attachment_path, content_hash};
    use crate::db::Database;
    use crate::migrations::AllowAllMigrations;
    use crate::test_support::scratch_database;

    fn store() -> (tempfile::TempDir, Database, SqliteEvidenceStore) {
        let (dir, mut database) = scratch_database();
        database
            .migrate(&AllowAllMigrations)
            .expect("the migrations apply");
        let store = SqliteEvidenceStore::new(&database, dir.path().join("attachments"));
        (dir, database, store)
    }

    fn append(kind: &'static str, facts: serde_json::Value) -> TimelineAppend {
        TimelineAppend { kind, facts }
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
    fn managed_file_evidence_stores_bytes_and_hash_metadata() {
        let (dir, database, store) = store();
        let content = b"proof bytes";
        let encoded = STANDARD.encode(content);
        let hash = content_hash(content);
        let item = store
            .attach_managed_file(
                "kan-p1",
                "ticket",
                "kan-t10",
                &encoded,
                append(
                    "evidence",
                    json!({
                        "action": "attached",
                        "evidence_kind": "managed_file",
                        "content_hash": hash,
                    }),
                ),
            )
            .expect("the managed file lands");

        assert_eq!(item.kind(), EvidenceKind::ManagedFile);
        assert_eq!(item.content_hash().expect("hash present").as_str(), hash);
        let path = attachment_path(
            &dir.path().join("attachments"),
            item.content_hash().unwrap(),
        );
        assert_eq!(std::fs::read(path).expect("bytes exist"), content);
        assert_eq!(
            timeline_rows(&database)
                .last()
                .cloned()
                .expect("timeline appended"),
            (
                "evidence".to_owned(),
                json!({
                    "action": "attached",
                    "evidence_kind": "managed_file",
                    "content_hash": hash,
                    "entity_kind": "ticket",
                    "entity_id": "kan-t10",
                    "id": item.id().value(),
                })
            )
        );
    }

    #[test]
    fn managed_file_read_verifies_the_hash() {
        let (_dir, _database, store) = store();
        let content = b"verify me";
        let encoded = STANDARD.encode(content);
        let item = store
            .attach_managed_file(
                "kan-p1",
                "ticket",
                "kan-t10",
                &encoded,
                append("evidence", json!({ "action": "attached" })),
            )
            .expect("the managed file lands");

        let bytes = store
            .read_managed_file(item.content_hash().expect("hash present"))
            .expect("the read verifies");
        assert_eq!(bytes, content);
    }

    #[test]
    fn managed_file_hash_mismatch_is_detected_on_read() {
        let (dir, _database, store) = store();
        let content = b"tamper target";
        let encoded = STANDARD.encode(content);
        let item = store
            .attach_managed_file(
                "kan-p1",
                "ticket",
                "kan-t10",
                &encoded,
                append("evidence", json!({ "action": "attached" })),
            )
            .expect("the managed file lands");
        let path = attachment_path(
            &dir.path().join("attachments"),
            item.content_hash().unwrap(),
        );
        std::fs::write(path, b"altered").expect("the bytes are tampered");

        let error = store
            .read_managed_file(item.content_hash().expect("hash present"))
            .expect_err("the mismatch is refused");
        assert!(error.message.contains("hash mismatch"));
    }

    #[test]
    fn repository_evidence_records_path_and_commit_without_copying_bytes() {
        let (dir, database, store) = store();
        let path = RelativePath::new("docs/spec.md").expect("the path validates");
        let commit = CommitIdentity::new("deadbeef").expect("the commit validates");
        let item = store
            .attach_repository(
                "kan-p1",
                "ticket",
                "kan-t10",
                &path,
                &commit,
                append(
                    "evidence",
                    json!({
                        "action": "attached",
                        "evidence_kind": "repository",
                        "relative_path": "docs/spec.md",
                        "commit_identity": "deadbeef",
                    }),
                ),
            )
            .expect("the repository evidence lands");

        assert_eq!(item.kind(), EvidenceKind::Repository);
        assert_eq!(
            item.relative_path().expect("path present").as_str(),
            "docs/spec.md"
        );
        assert_eq!(
            item.commit_identity().expect("commit present").as_str(),
            "deadbeef"
        );
        assert!(
            !dir.path().join("attachments").join("docs").exists(),
            "repository evidence must never copy content"
        );
        assert_eq!(
            timeline_rows(&database)
                .last()
                .cloned()
                .expect("timeline appended"),
            (
                "evidence".to_owned(),
                json!({
                    "action": "attached",
                    "evidence_kind": "repository",
                    "relative_path": "docs/spec.md",
                    "commit_identity": "deadbeef",
                    "entity_kind": "ticket",
                    "entity_id": "kan-t10",
                    "id": item.id().value(),
                })
            )
        );
    }

    #[test]
    fn listing_evidence_appends_a_timeline_event() {
        let (_dir, database, store) = store();
        let encoded = STANDARD.encode(b"listed");
        store
            .attach_managed_file(
                "kan-p1",
                "ticket",
                "kan-t10",
                &encoded,
                append("evidence", json!({ "action": "attached" })),
            )
            .expect("the managed file lands");

        let listed = store
            .list(
                &EvidenceFilter {
                    project_id: "kan-p1".to_owned(),
                    entity_kind: Some("ticket".to_owned()),
                    entity_id: Some("kan-t10".to_owned()),
                },
                append(
                    "evidence",
                    json!({
                        "action": "listed",
                        "entity_kind": "ticket",
                        "entity_id": "kan-t10",
                    }),
                ),
            )
            .expect("the list serves");

        assert_eq!(listed.len(), 1);
        assert_eq!(
            timeline_rows(&database)
                .last()
                .cloned()
                .expect("timeline appended"),
            (
                "evidence".to_owned(),
                json!({
                    "action": "listed",
                    "entity_kind": "ticket",
                    "entity_id": "kan-t10",
                })
            )
        );
    }

    #[test]
    fn deleting_evidence_is_refused_by_the_schema() {
        let (_dir, _database, store) = store();
        let encoded = STANDARD.encode(b"immutable");
        store
            .attach_managed_file(
                "kan-p1",
                "ticket",
                "kan-t10",
                &encoded,
                append("evidence", json!({ "action": "attached" })),
            )
            .expect("the managed file lands");

        let outcome = store
            .lock()
            .execute("DELETE FROM evidence_items WHERE id = 1", []);

        let error = outcome.expect_err("the schema must refuse deletes");
        assert!(
            error.to_string().contains("append-only"),
            "the refusal should say append-only, got: {error}"
        );
    }
}
