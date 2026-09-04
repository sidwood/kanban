-- 0004 comments
--
-- Comments attach to timeline-visible entities with immutable
-- revision history (DR-AE-02). Revisions are append-only; only a
-- new revision changes the current text.

CREATE TABLE comments (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    project_id   TEXT NOT NULL,
    entity_kind  TEXT NOT NULL,
    entity_id    TEXT NOT NULL,
    version      INTEGER NOT NULL CHECK (version > 0),
    created_at   TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

CREATE TABLE comment_revisions (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    comment_id   INTEGER NOT NULL REFERENCES comments(id),
    revision     INTEGER NOT NULL CHECK (revision > 0),
    text         TEXT NOT NULL,
    recorded_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    UNIQUE (comment_id, revision)
);

CREATE INDEX idx_comment_revisions_comment_revision
    ON comment_revisions (comment_id, revision);

CREATE TRIGGER comment_revisions_append_only_update
BEFORE UPDATE ON comment_revisions
BEGIN
    SELECT RAISE(ABORT, 'comment_revisions is append-only');
END;

CREATE TRIGGER comment_revisions_append_only_delete
BEFORE DELETE ON comment_revisions
BEGIN
    SELECT RAISE(ABORT, 'comment_revisions is append-only');
END;
