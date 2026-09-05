-- 0025 lanes
--
-- Lanes are durable execution slots (KAN-S6-US2): each holds at most
-- one active Ticket and each Workspace belongs to at most one Lane
-- (DR-LW-02, DR-LW-03), both enforced at the row level by UNIQUE
-- constraints. The Seed Workspace is never claimed — the application
-- layer refuses and records every attempt (DR-LW-07). Nothing in
-- Kanban deletes a Lane.

CREATE TABLE lanes (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    project_id   INTEGER NOT NULL REFERENCES projects (id),
    workspace_id INTEGER REFERENCES workspaces (id),
    ticket_id    INTEGER REFERENCES tickets (id),
    version      INTEGER NOT NULL CHECK (version > 0),
    created_at   TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    UNIQUE (workspace_id),
    UNIQUE (ticket_id)
);

CREATE TRIGGER lanes_refuse_delete
BEFORE DELETE ON lanes
BEGIN
    SELECT RAISE(ABORT, 'lanes are durable execution slots; they are never deleted');
END;
