-- 0002 initiatives
--
-- Initiatives are non-nested folders (CONTEXT.md): the table has no
-- parent column, so nesting cannot be expressed. Nothing in Kanban
-- is deleted; the trigger refuses deletes at the schema level and
-- archiving keeps every column.

CREATE TABLE initiatives (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    name        TEXT NOT NULL,
    archived    INTEGER NOT NULL DEFAULT 0 CHECK (archived IN (0, 1)),
    version     INTEGER NOT NULL CHECK (version > 0),
    created_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    archived_at TEXT
);

CREATE TRIGGER initiatives_refuse_delete
BEFORE DELETE ON initiatives
BEGIN
    SELECT RAISE(ABORT, 'initiatives are archived, never deleted');
END;
