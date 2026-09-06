-- 0033 runs
--
-- Runs (CONTEXT.md, DR-EP-04): one execution attempt behind a claimed
-- Dispatch Request, freezing the requested and effective profile
-- snapshots at mint so a later catalogue change never rewrites what
-- ran (DR-EP-05). The fallback path is stored as a JSON array of the
-- names the walk touched, requested first. The partial unique index
-- keeps one executing run per request; a retry in a fresh run is
-- recovery's to mint after settlement exists.

CREATE TABLE runs (
    id INTEGER PRIMARY KEY,
    project_id INTEGER NOT NULL REFERENCES projects(id),
    ticket_id INTEGER NOT NULL REFERENCES tickets(id),
    dispatch_request_id INTEGER NOT NULL REFERENCES dispatch_requests(id),
    status TEXT NOT NULL CHECK (status IN ('executing')),
    requested_name TEXT NOT NULL CHECK (length(trim(requested_name)) > 0),
    requested_harness TEXT NOT NULL CHECK (length(trim(requested_harness)) > 0),
    requested_model TEXT NOT NULL CHECK (length(trim(requested_model)) > 0),
    requested_effort TEXT NOT NULL CHECK (length(trim(requested_effort)) > 0),
    requested_usage_pool TEXT NOT NULL CHECK (length(trim(requested_usage_pool)) > 0),
    effective_name TEXT NOT NULL CHECK (length(trim(effective_name)) > 0),
    effective_harness TEXT NOT NULL CHECK (length(trim(effective_harness)) > 0),
    effective_model TEXT NOT NULL CHECK (length(trim(effective_model)) > 0),
    effective_effort TEXT NOT NULL CHECK (length(trim(effective_effort)) > 0),
    effective_usage_pool TEXT NOT NULL CHECK (length(trim(effective_usage_pool)) > 0),
    fallback INTEGER NOT NULL CHECK (fallback IN (0, 1)),
    fallback_path TEXT NOT NULL DEFAULT '[]',
    created_at INTEGER NOT NULL CHECK (created_at >= 0),
    version INTEGER NOT NULL CHECK (version > 0)
);

CREATE UNIQUE INDEX runs_one_executing_per_request
    ON runs(dispatch_request_id)
    WHERE status = 'executing';

CREATE INDEX runs_by_project ON runs(project_id);
