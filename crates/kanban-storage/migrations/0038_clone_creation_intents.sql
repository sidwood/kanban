-- Written before invoking the clone tool, outside the command's mutation.
-- A pending intent is uncertainty after interruption, never a success claim.
CREATE TABLE clone_creation_intents (
    idempotency_key TEXT PRIMARY KEY,
    project_id INTEGER NOT NULL REFERENCES projects(id),
    path TEXT NOT NULL,
    branch TEXT NOT NULL,
    source TEXT NOT NULL,
    workspace_id INTEGER REFERENCES workspaces(id)
);
CREATE INDEX pending_clone_creation_by_project
ON clone_creation_intents(project_id) WHERE workspace_id IS NULL;
