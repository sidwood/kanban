-- 0031 dispatch requests
--
-- Durable Dispatch Requests (CONTEXT.md, DR-EP-08, DR-HB-14): one
-- row per request, queued while capacity is unavailable and claimed
-- by exactly one concurrent claimant. Profile families, priority,
-- and readiness are snapshotted at enqueue so a later catalogue or
-- Ticket change cannot rewrite the queue or the capacity dimensions
-- a request will draw. The partial unique index keeps one open
-- request per Ticket, which is how duplicate dispatch is refused at
-- the row level as well as in the domain.

CREATE TABLE dispatch_requests (
    id INTEGER PRIMARY KEY,
    project_id INTEGER NOT NULL REFERENCES projects(id),
    ticket_id INTEGER NOT NULL REFERENCES tickets(id),
    status TEXT NOT NULL CHECK (status IN ('queued', 'claimed')),
    priority TEXT NOT NULL CHECK (priority IN ('urgent', 'high', 'normal', 'low')),
    ready INTEGER NOT NULL CHECK (ready IN (0, 1)),
    harness TEXT NOT NULL CHECK (length(trim(harness)) > 0),
    model TEXT NOT NULL CHECK (length(trim(model)) > 0),
    usage_pool TEXT NOT NULL CHECK (length(trim(usage_pool)) > 0),
    created_at INTEGER NOT NULL CHECK (created_at >= 0),
    version INTEGER NOT NULL CHECK (version > 0)
);

CREATE UNIQUE INDEX dispatch_requests_open_ticket
    ON dispatch_requests(ticket_id)
    WHERE status IN ('queued', 'claimed');
