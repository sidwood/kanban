-- 0008 evidence
--
-- Evidence metadata is append-only (DR-AE-11, DR-AE-13). Managed-file
-- bytes live under attachments/ in managed application data; this
-- table stores hashes and repository references only.

CREATE TABLE evidence_items (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    project_id      TEXT NOT NULL,
    entity_kind     TEXT NOT NULL,
    entity_id       TEXT NOT NULL,
    kind            TEXT NOT NULL,
    content_hash    TEXT,
    relative_path   TEXT,
    commit_identity TEXT,
    recorded_at     TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

CREATE INDEX evidence_items_project_idx
    ON evidence_items (project_id, id);

CREATE INDEX evidence_items_entity_idx
    ON evidence_items (project_id, entity_kind, entity_id, id);

CREATE TRIGGER evidence_items_append_only_update
BEFORE UPDATE ON evidence_items
BEGIN
    SELECT RAISE(ABORT, 'evidence_items is append-only');
END;

CREATE TRIGGER evidence_items_append_only_delete
BEFORE DELETE ON evidence_items
BEGIN
    SELECT RAISE(ABORT, 'evidence_items is append-only');
END;
