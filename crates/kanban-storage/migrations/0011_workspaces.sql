-- 0011 workspaces
--
-- Registered working copies with observed git state and health
-- (KAN-S6-US1). Nothing in Kanban deletes a Workspace.

CREATE TABLE workspaces (
    id                   INTEGER PRIMARY KEY AUTOINCREMENT,
    project_id           INTEGER NOT NULL REFERENCES projects (id),
    path                 TEXT NOT NULL,
    is_seed              INTEGER NOT NULL DEFAULT 0 CHECK (is_seed IN (0, 1)),
    retired              INTEGER NOT NULL DEFAULT 0 CHECK (retired IN (0, 1)),
    lane_id              INTEGER,
    health               TEXT NOT NULL CHECK (health IN (
        'available', 'assigned', 'dirty', 'missing', 'retired'
    )),
    repository_identity  TEXT,
    branch               TEXT,
    head                 TEXT,
    working_tree_clean   INTEGER CHECK (working_tree_clean IN (0, 1)),
    version              INTEGER NOT NULL CHECK (version > 0),
    created_at           TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    UNIQUE (project_id, path)
);

CREATE TRIGGER workspaces_refuse_delete
BEFORE DELETE ON workspaces
BEGIN
    SELECT RAISE(ABORT, 'workspaces are retired, never deleted');
END;
