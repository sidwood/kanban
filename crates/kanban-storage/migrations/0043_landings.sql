-- Specs own one integration branch and Workspace; landings are
-- guarded and recorded, never ad-hoc.
CREATE TABLE spec_integrations (
    spec_id INTEGER PRIMARY KEY REFERENCES specs(id),
    project_id INTEGER NOT NULL REFERENCES projects(id),
    branch TEXT NOT NULL,
    workspace_path TEXT NOT NULL UNIQUE,
    workspace_id INTEGER NOT NULL UNIQUE REFERENCES workspaces(id),
    review_approved INTEGER NOT NULL DEFAULT 0 CHECK (review_approved IN (0, 1)),
    approved_tip TEXT,
    base_tip TEXT NOT NULL,
    CHECK (review_approved = 0 OR approved_tip IS NOT NULL),
    UNIQUE(project_id, branch)
);

CREATE TABLE landing_intents (
    idempotency_key TEXT PRIMARY KEY,
    project_id INTEGER NOT NULL REFERENCES projects(id),
    from_path TEXT NOT NULL,
    into_path TEXT NOT NULL,
    draft TEXT NOT NULL CHECK (json_valid(draft)),
    completed INTEGER NOT NULL DEFAULT 0 CHECK (completed IN (0, 1))
);

CREATE TABLE landings (
    id INTEGER PRIMARY KEY,
    project_id INTEGER NOT NULL REFERENCES projects(id),
    kind TEXT NOT NULL CHECK (kind IN ('lane', 'seed', 'standalone_bug')),
    from_path TEXT NOT NULL,
    into_path TEXT NOT NULL,
    from_branch TEXT NOT NULL,
    into_branch TEXT NOT NULL,
    from_tip TEXT NOT NULL,
    into_tip TEXT NOT NULL,
    landed_tip TEXT NOT NULL,
    spec_id INTEGER REFERENCES specs(id),
    ticket_id INTEGER REFERENCES tickets(id),
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);
