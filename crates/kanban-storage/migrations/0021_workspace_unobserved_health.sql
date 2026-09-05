-- 0021 workspace unobserved health
--
-- KAN-T99 separates observation failure from a genuinely dirty
-- worktree: a present Workspace whose git status could not be read
-- holds the `unobserved` health, which claims neither a clean nor a
-- dirty tree and never counts as reuse capacity. The health check
-- gains that state.
--
-- Rows whose stored health asserts a tree verdict the record lacks
-- are normalised to `unobserved` during the rebuild, so no read path
-- can restore a missing clean flag as clean.
--
-- The rebuild drops the workspaces table while other tables still
-- reference it; the migration runner applies schema changes with
-- foreign key enforcement off and validates the final state with a
-- foreign key check before committing.

CREATE TABLE workspaces_rebuilt (
    id                   INTEGER PRIMARY KEY AUTOINCREMENT,
    project_id           INTEGER NOT NULL REFERENCES projects (id),
    path                 TEXT NOT NULL,
    is_seed              INTEGER NOT NULL DEFAULT 0 CHECK (is_seed IN (0, 1)),
    retired              INTEGER NOT NULL DEFAULT 0 CHECK (retired IN (0, 1)),
    lane_id              INTEGER,
    health               TEXT NOT NULL CHECK (health IN (
        'available', 'assigned', 'dirty', 'missing', 'retired', 'unobserved'
    )),
    repository_identity  TEXT,
    branch               TEXT,
    head                 TEXT,
    working_tree_clean   INTEGER CHECK (working_tree_clean IN (0, 1)),
    version              INTEGER NOT NULL CHECK (version > 0),
    created_at           TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    unique_unlanded_commits INTEGER CHECK (unique_unlanded_commits IN (0, 1)),
    detached             INTEGER NOT NULL DEFAULT 0 CHECK (detached IN (0, 1)),
    UNIQUE (project_id, path)
);

INSERT INTO workspaces_rebuilt
    (id, project_id, path, is_seed, retired, lane_id, health,
     repository_identity, branch, head, working_tree_clean, version,
     created_at, unique_unlanded_commits, detached)
SELECT id, project_id, path, is_seed, retired, lane_id,
       CASE WHEN health IN ('available', 'dirty') AND working_tree_clean IS NULL
            THEN 'unobserved' ELSE health END,
       repository_identity, branch, head, working_tree_clean, version,
       created_at, unique_unlanded_commits, detached
FROM workspaces;

-- The delete-refusal trigger would abort the table rebuild's implicit
-- row deletes, so it drops first and returns unchanged afterwards.
DROP TRIGGER workspaces_refuse_delete;

DROP TABLE workspaces;

ALTER TABLE workspaces_rebuilt RENAME TO workspaces;

CREATE TRIGGER workspaces_refuse_delete
BEFORE DELETE ON workspaces
BEGIN
    SELECT RAISE(ABORT, 'workspaces are retired, never deleted');
END;
