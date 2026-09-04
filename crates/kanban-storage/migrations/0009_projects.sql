-- 0009 projects
--
-- Projects anchor to exactly one target repository, Seed Workspace,
-- default branch, and exclusive named Herdr session (CONTEXT.md).
-- Codes are globally unique and immutable, and session names stay
-- exclusive even after archiving, which is why both uniqueness
-- constraints cover every row, archived ones included. Nothing in
-- Kanban is deleted: the trigger refuses deletes at the schema level
-- and archiving keeps every column, including the number counters.

CREATE TABLE projects (
    id             INTEGER PRIMARY KEY AUTOINCREMENT,
    code           TEXT NOT NULL UNIQUE,
    name           TEXT NOT NULL,
    repository     TEXT NOT NULL,
    seed_workspace TEXT NOT NULL,
    default_branch TEXT NOT NULL,
    herdr_session  TEXT NOT NULL UNIQUE,
    initiative_id  INTEGER REFERENCES initiatives (id),
    archived       INTEGER NOT NULL DEFAULT 0 CHECK (archived IN (0, 1)),
    plan_counter   INTEGER NOT NULL DEFAULT 0 CHECK (plan_counter >= 0),
    spec_counter   INTEGER NOT NULL DEFAULT 0 CHECK (spec_counter >= 0),
    ticket_counter INTEGER NOT NULL DEFAULT 0 CHECK (ticket_counter >= 0),
    version        INTEGER NOT NULL CHECK (version > 0),
    created_at     TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    archived_at    TEXT
);

CREATE TRIGGER projects_refuse_delete
BEFORE DELETE ON projects
BEGIN
    SELECT RAISE(ABORT, 'projects are archived, never deleted');
END;
