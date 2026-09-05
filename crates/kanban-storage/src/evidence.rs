//! The SQLite implementation of the evidence storage port: metadata
//! rows, hash-backed attachment bytes, and timeline appends in the
//! same transaction as every change.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use kanban_app::{EvidenceFilter, EvidenceStore, TimelineEnvelope, TimelineFacts};
use kanban_domain::{
    CommitIdentity, ContentHash, EvidenceId, EvidenceItem, EvidenceKind, EvidenceShape,
    RelativePath,
};
use kanban_dto::{ApiError, TimelineEntityKind, TimelineEntityRef};
use rusqlite::params;
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::db::{ConnectionHandle, Database, WriteSpan};
use crate::timeline::insert_event;

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

    fn lock(&self) -> parking_lot::ReentrantMutexGuard<'_, rusqlite::Connection> {
        self.conn.lock()
    }
}

impl EvidenceStore for SqliteEvidenceStore {
    fn attach_managed_file(
        &self,
        project_id: u64,
        entity_kind: &str,
        entity_id: &str,
        content_base64: &str,
        facts: TimelineFacts,
    ) -> Result<EvidenceItem, ApiError> {
        let (project_id, entity_kind, entity_id) =
            canonicalise_identities(project_id, entity_kind, entity_id)?;
        let content = STANDARD
            .decode(content_base64)
            .map_err(|error| ApiError::invalid_request(&error.to_string()))?;
        let hash = content_hash(&content);
        let content_hash = ContentHash::new(&hash)
            .map_err(|error| ApiError::invalid_request(&error.to_string()))?;
        write_attachment(&self.attachments_dir, &content_hash, &content)?;
        self.insert_row(
            EvidenceShape {
                project_id,
                entity_kind,
                entity_id,
                kind: EvidenceKind::ManagedFile,
                content_hash: Some(content_hash),
                relative_path: None,
                commit_identity: None,
            },
            facts,
        )
    }

    fn attach_repository(
        &self,
        project_id: u64,
        entity_kind: &str,
        entity_id: &str,
        relative_path: &RelativePath,
        commit_identity: &CommitIdentity,
        facts: TimelineFacts,
    ) -> Result<EvidenceItem, ApiError> {
        let (project_id, entity_kind, entity_id) =
            canonicalise_identities(project_id, entity_kind, entity_id)?;
        self.insert_row(
            EvidenceShape {
                project_id,
                entity_kind,
                entity_id,
                kind: EvidenceKind::Repository,
                content_hash: None,
                relative_path: Some(relative_path.clone()),
                commit_identity: Some(commit_identity.clone()),
            },
            facts,
        )
    }

    fn list(
        &self,
        filter: &EvidenceFilter,
        envelope: TimelineEnvelope,
    ) -> Result<Vec<EvidenceItem>, ApiError> {
        let conn = self.lock();
        let span = WriteSpan::begin(&conn).map_err(internal)?;
        let items = query_rows(&span, filter)?;
        insert_event(&span, &envelope).map_err(internal)?;
        span.commit().map_err(internal)?;
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
        facts: TimelineFacts,
    ) -> Result<EvidenceItem, ApiError> {
        let conn = self.lock();
        let span = WriteSpan::begin(&conn).map_err(internal)?;
        let kind_wire = evidence_kind_wire(shape.kind);
        span.execute(
            "INSERT INTO evidence_items (
                     project_id, entity_kind, entity_id, kind,
                     content_hash, relative_path, commit_identity
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                shape.project_id.to_string(),
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
            span.last_insert_rowid()
                .try_into()
                .map_err(|_| ApiError::internal("the evidence identity overflowed"))?,
        );
        let item = EvidenceItem::restore(id, shape)
            .map_err(|error| ApiError::internal(&error.to_string()))?;
        let mut detail = facts.facts;
        let detail_object = detail
            .as_object_mut()
            .ok_or_else(|| ApiError::internal("timeline facts must be a JSON object"))?;
        detail_object.insert("id".to_owned(), Value::from(id.value()));
        detail_object.insert("entity_kind".to_owned(), Value::from(item.entity_kind()));
        detail_object.insert("entity_id".to_owned(), Value::from(item.entity_id()));
        let envelope = TimelineEnvelope::project(
            item.project_id(),
            facts.kind,
            Some(TimelineEntityRef {
                kind: TimelineEntityKind::parse(item.entity_kind()).ok_or_else(|| {
                    ApiError::internal("a stored evidence entity kind is corrupt")
                })?,
                id: item.entity_id().to_owned(),
            }),
            detail,
        );
        insert_event(&span, &envelope).map_err(internal)?;
        span.commit().map_err(internal)?;
        Ok(item)
    }
}

/// The lowercase SHA-256 digest of `content`.
pub fn content_hash(content: &[u8]) -> String {
    let digest = Sha256::digest(content);
    format!("{:x}", digest)
}

/// Write managed attachment bytes before the metadata row lands.
///
/// Blobs are content-addressed, so a file already under the hash name
/// with matching bytes is this attachment and is kept. Mismatching
/// bytes are a torn or corrupt blob, never a second payload, and are
/// replaced. The bytes go to a sibling temporary file that is synced
/// and renamed onto the hash name, so an interrupted write leaves the
/// previous file or the complete new one under the authentic name and
/// never a torn blob that would block reattachment.
fn write_attachment(
    attachments_dir: &Path,
    hash: &ContentHash,
    content: &[u8],
) -> Result<(), ApiError> {
    fs::create_dir_all(attachments_dir).map_err(|error| ApiError::internal(&error.to_string()))?;
    let path = attachment_path(attachments_dir, hash);
    let authentic = fs::read(&path)
        .ok()
        .is_some_and(|existing| content_hash(&existing) == hash.as_str());
    if authentic {
        return Ok(());
    }
    let staging = staging_path(attachments_dir, hash);
    let staged = fs::File::create(&staging).and_then(|mut file| {
        file.write_all(content)?;
        file.sync_all()?;
        fs::rename(&staging, &path)
    });
    staged.map_err(|error| {
        // Best effort: a stale staging file is harmless, and the
        // original failure is the one worth reporting.
        let _ = fs::remove_file(&staging);
        ApiError::internal(&error.to_string())
    })
}

/// The sibling path an in-flight attachment write goes to. It sits in
/// the attachments directory so the final rename stays on one file
/// system, and it is unique per process and call so two attaches of
/// one payload never share a staging file.
fn staging_path(attachments_dir: &Path, hash: &ContentHash) -> PathBuf {
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
    attachments_dir.join(format!(
        "{}.{}-{sequence}.staging",
        hash.as_str(),
        std::process::id()
    ))
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
    let mut bindings: Vec<String> = vec![filter.project_id.to_string()];

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
    // A non-numeric scope is a legacy row migration 0011 missed;
    // refusing it beats guessing which Project owns it.
    let project_id: u64 = row.get::<_, String>(1)?.parse().map_err(|_| {
        rusqlite::Error::FromSqlConversionFailure(
            1,
            rusqlite::types::Type::Text,
            Box::new(CorruptEvidence),
        )
    })?;
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

fn internal(error: impl std::fmt::Display) -> ApiError {
    ApiError::internal(&error.to_string())
}

fn canonicalise_identities(
    project_id: u64,
    entity_kind: &str,
    entity_id: &str,
) -> Result<(u64, String, String), ApiError> {
    if !kanban_domain::is_entity_kind(entity_kind) {
        return Err(ApiError::invalid_request(&format!(
            "unknown entity kind `{entity_kind}`"
        )));
    }
    let entity_id = entity_id.trim();
    if entity_id.is_empty() {
        return Err(ApiError::invalid_request(
            "an entity identity cannot be blank",
        ));
    }
    Ok((project_id, entity_kind.to_owned(), entity_id.to_owned()))
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
    use kanban_app::{EvidenceFilter, EvidenceStore, TimelineEnvelope, TimelineFacts};
    use kanban_domain::{CommitIdentity, ContentHash, EvidenceKind, RelativePath};
    use kanban_dto::{TimelineEntityKind, TimelineEntityRef, TimelineEventKind, TimelineScope};
    use serde_json::json;

    use super::{SqliteEvidenceStore, attachment_path, content_hash};
    use crate::db::Database;
    use crate::migrations::AllowAllMigrations;
    use crate::test_support::scratch_database;
    use crate::timeline::{TimelineFilter, TimelineRow};

    fn store() -> (tempfile::TempDir, Database, SqliteEvidenceStore) {
        let (dir, mut database) = scratch_database();
        database
            .migrate(&AllowAllMigrations)
            .expect("the migrations apply");
        let store = SqliteEvidenceStore::new(&database, dir.path().join("attachments"));
        (dir, database, store)
    }

    fn append(_kind: &'static str, facts: serde_json::Value) -> TimelineFacts {
        TimelineFacts {
            kind: TimelineEventKind::Evidence,
            facts,
        }
    }

    fn list_append(entity: Option<(&str, &str)>, detail: serde_json::Value) -> TimelineEnvelope {
        TimelineEnvelope::project(
            1,
            TimelineEventKind::Evidence,
            entity.map(|(kind, id)| TimelineEntityRef {
                kind: TimelineEntityKind::parse(kind).expect("the test kind is valid"),
                id: id.to_owned(),
            }),
            detail,
        )
    }

    /// The Project timeline as the query surface reads it, so every
    /// test sees the envelope columns and not only the detail.
    fn timeline_rows(database: &Database) -> Vec<TimelineRow> {
        database
            .query_timeline(&TimelineFilter::of(TimelineScope::Project(1)))
            .expect("the timeline is readable")
    }

    fn last_event(database: &Database) -> TimelineRow {
        timeline_rows(database).pop().expect("timeline appended")
    }

    fn envelope(row: &TimelineRow) -> (Option<&str>, Option<&str>) {
        (row.entity_kind.as_deref(), row.entity_id.as_deref())
    }

    #[test]
    fn managed_file_evidence_stores_bytes_and_hash_metadata() {
        let (dir, database, store) = store();
        let content = b"proof bytes";
        let encoded = STANDARD.encode(content);
        let hash = content_hash(content);
        let item = store
            .attach_managed_file(
                1,
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
        let event = last_event(&database);
        assert_eq!(event.kind, "evidence");
        assert_eq!(
            event.detail,
            json!({
                "action": "attached",
                "evidence_kind": "managed_file",
                "content_hash": hash,
                "entity_kind": "ticket",
                "entity_id": "kan-t10",
                "id": item.id().value(),
            })
        );
        assert_eq!(
            envelope(&event),
            (Some("ticket"), Some("kan-t10")),
            "the attach event references the subject entity"
        );
    }

    #[test]
    fn attach_event_appears_when_timeline_is_filtered_to_the_subject_entity() {
        let (_dir, database, store) = store();
        let content = b"subject-filter proof";
        let encoded = STANDARD.encode(content);
        store
            .attach_managed_file(
                1,
                "ticket",
                "kan-t10",
                &encoded,
                append(
                    "evidence",
                    json!({
                        "action": "attached",
                        "evidence_kind": "managed_file",
                        "content_hash": content_hash(content),
                    }),
                ),
            )
            .expect("the managed file lands");

        let rows = database
            .query_timeline(&TimelineFilter {
                entity_kind: Some("ticket".to_owned()),
                entity_id: Some("kan-t10".to_owned()),
                ..TimelineFilter::of(TimelineScope::Project(1))
            })
            .expect("the subject filter applies");

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].kind, "evidence");
        assert_eq!(rows[0].detail["action"], json!("attached"));
        assert_eq!(
            envelope(&rows[0]),
            (Some("ticket"), Some("kan-t10")),
            "subject-filtered timelines include the attach event"
        );
    }

    #[test]
    fn managed_file_read_verifies_the_hash() {
        let (_dir, _database, store) = store();
        let content = b"verify me";
        let encoded = STANDARD.encode(content);
        let item = store
            .attach_managed_file(
                1,
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
                1,
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

    /// A hash-named file whose bytes do not match its name is a torn
    /// or corrupt blob, not a second payload: reattaching the authentic
    /// bytes must heal it rather than fail forever on a collision.
    #[test]
    fn a_torn_attachment_is_replaced_when_the_authentic_bytes_reattach() {
        let (dir, _database, store) = store();
        let attachments = dir.path().join("attachments");
        let content = b"proof bytes";
        let hash = ContentHash::new(&content_hash(content)).expect("the digest validates");
        std::fs::create_dir_all(&attachments).expect("the attachments dir exists");
        std::fs::write(attachment_path(&attachments, &hash), &content[..4])
            .expect("the torn blob is planted");

        let item = store
            .attach_managed_file(
                1,
                "ticket",
                "kan-t10",
                &STANDARD.encode(content),
                append("evidence", json!({ "action": "attached" })),
            )
            .expect("reattaching the authentic bytes heals the torn blob");

        assert_eq!(item.content_hash().expect("hash present"), &hash);
        assert_eq!(
            std::fs::read(attachment_path(&attachments, &hash)).expect("bytes exist"),
            content
        );
        assert_eq!(
            store.read_managed_file(&hash).expect("the read verifies"),
            content
        );
        let entries: Vec<_> = std::fs::read_dir(&attachments)
            .expect("the attachments dir lists")
            .map(|entry| entry.expect("the entry reads").file_name())
            .collect();
        assert_eq!(
            entries,
            vec![std::ffi::OsString::from(hash.as_str())],
            "the write leaves no temporary file behind"
        );
    }

    #[test]
    fn repository_evidence_records_path_and_commit_without_copying_bytes() {
        let (dir, database, store) = store();
        let path = RelativePath::new("docs/spec.md").expect("the path validates");
        let commit = CommitIdentity::new("deadbeef").expect("the commit validates");
        let item = store
            .attach_repository(
                1,
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
        let event = last_event(&database);
        assert_eq!(event.kind, "evidence");
        assert_eq!(
            event.detail,
            json!({
                "action": "attached",
                "evidence_kind": "repository",
                "relative_path": "docs/spec.md",
                "commit_identity": "deadbeef",
                "entity_kind": "ticket",
                "entity_id": "kan-t10",
                "id": item.id().value(),
            })
        );
    }

    #[test]
    fn listing_evidence_appends_a_timeline_event() {
        let (_dir, database, store) = store();
        let encoded = STANDARD.encode(b"listed");
        store
            .attach_managed_file(
                1,
                "ticket",
                "kan-t10",
                &encoded,
                append("evidence", json!({ "action": "attached" })),
            )
            .expect("the managed file lands");

        let listed = store
            .list(
                &EvidenceFilter {
                    project_id: 1,
                    entity_kind: Some("ticket".to_owned()),
                    entity_id: Some("kan-t10".to_owned()),
                },
                list_append(
                    Some(("ticket", "kan-t10")),
                    json!({
                        "action": "listed",
                        "entity_kind": "ticket",
                        "entity_id": "kan-t10",
                    }),
                ),
            )
            .expect("the list serves");

        assert_eq!(listed.len(), 1);
        let event = last_event(&database);
        assert_eq!(event.kind, "evidence");
        assert_eq!(
            event.detail,
            json!({
                "action": "listed",
                "entity_kind": "ticket",
                "entity_id": "kan-t10",
            })
        );
        assert_eq!(
            envelope(&event),
            (Some("ticket"), Some("kan-t10")),
            "a list filtered to one entity references that entity"
        );
    }

    #[test]
    fn listing_a_whole_project_appends_an_unreferenced_timeline_event() {
        let (_dir, database, store) = store();

        store
            .list(
                &EvidenceFilter {
                    project_id: 1,
                    entity_kind: None,
                    entity_id: None,
                },
                list_append(
                    None,
                    json!({
                        "action": "listed",
                        "entity_kind": null,
                        "entity_id": null,
                    }),
                ),
            )
            .expect("the list serves");

        let event = last_event(&database);
        assert_eq!(event.kind, "evidence");
        assert_eq!(
            event.detail,
            json!({
                "action": "listed",
                "entity_kind": null,
                "entity_id": null,
            })
        );
        assert_eq!(
            envelope(&event),
            (None, None),
            "a Project-wide list leaves both envelope columns empty"
        );
    }

    #[test]
    fn listing_by_entity_kind_alone_leaves_the_envelope_unreferenced() {
        let (_dir, database, store) = store();

        store
            .list(
                &EvidenceFilter {
                    project_id: 1,
                    entity_kind: Some("ticket".to_owned()),
                    entity_id: None,
                },
                list_append(
                    None,
                    json!({
                        "action": "listed",
                        "entity_kind": "ticket",
                        "entity_id": null,
                    }),
                ),
            )
            .expect("the list serves");

        assert_eq!(
            envelope(&last_event(&database)),
            (None, None),
            "half a reference is never written: the decoder refuses it"
        );
    }

    fn attached_store() -> (tempfile::TempDir, Database, SqliteEvidenceStore) {
        let (dir, database, store) = store();
        let encoded = STANDARD.encode(b"immutable");
        store
            .attach_managed_file(
                1,
                "ticket",
                "kan-t10",
                &encoded,
                append("evidence", json!({ "action": "attached" })),
            )
            .expect("the managed file lands");
        (dir, database, store)
    }

    #[test]
    fn updating_evidence_is_refused_by_the_schema() {
        let (_dir, _database, store) = attached_store();

        let outcome = store.lock().execute(
            "UPDATE evidence_items SET entity_id = 'tampered' WHERE id = 1",
            [],
        );

        let error = outcome.expect_err("the schema must refuse updates");
        assert!(
            error.to_string().contains("append-only"),
            "the refusal should say append-only, got: {error}"
        );
    }

    #[test]
    fn replacing_evidence_is_refused_by_the_schema() {
        let (_dir, _database, store) = attached_store();

        let outcome = store.lock().execute(
            "INSERT OR REPLACE INTO evidence_items
                 (id, project_id, entity_kind, entity_id, kind)
             VALUES (1, '1', 'ticket', 'tampered', 'repository')",
            [],
        );

        let error = outcome.expect_err("the schema must refuse replaces");
        assert!(
            error.to_string().contains("append-only"),
            "the refusal should say append-only, got: {error}"
        );
        let entity_id: String = store
            .lock()
            .query_row(
                "SELECT entity_id FROM evidence_items WHERE id = 1",
                [],
                |row| row.get(0),
            )
            .expect("the original row is still readable");
        assert_eq!(entity_id, "kan-t10", "the row must not be mutated");
    }

    #[test]
    fn deleting_evidence_is_refused_by_the_schema() {
        let (_dir, _database, store) = attached_store();

        let outcome = store
            .lock()
            .execute("DELETE FROM evidence_items WHERE id = 1", []);

        let error = outcome.expect_err("the schema must refuse deletes");
        assert!(
            error.to_string().contains("append-only"),
            "the refusal should say append-only, got: {error}"
        );
    }

    #[test]
    fn blank_identities_refuse_managed_file_without_blob_or_row() {
        let (dir, database, store) = store();
        let attachments = dir.path().join("attachments");
        let encoded = STANDARD.encode(b"proof");
        let facts = append("evidence", json!({ "action": "attached" }));

        for entity_id in ["   ", ""] {
            let error = store
                .attach_managed_file(1, "ticket", entity_id, &encoded, facts.clone())
                .expect_err("blank identities are refused");
            assert_eq!(error.code, kanban_dto::ErrorCode::InvalidRequest);
        }

        assert!(
            !attachments.exists(),
            "a refused attach must not create the attachments directory"
        );
        let row_count: i64 = database
            .connection()
            .query_row("SELECT COUNT(*) FROM evidence_items", [], |row| row.get(0))
            .expect("the table is readable");
        assert_eq!(row_count, 0, "a refused attach must not persist a row");
        assert!(
            timeline_rows(&database).is_empty(),
            "a refused attach must not append a timeline event"
        );
    }
}
