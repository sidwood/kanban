-- 0034 capabilities
--
-- Run-scoped capabilities (CONTEXT.md, DR-HB-17, DR-SS-14): one row
-- per won Dispatch Request claim, minted inside that claim's
-- transaction so a claim and the authority it grants land together
-- or not at all. The row binds the Ticket, the Lane the run executes
-- in, the role, the reviewer slot a reviewer occupies, and the
-- canonical JSON array of permitted MCP operations. Status is active
-- until run settlement expires it; settled is one-way, and the
-- UNIQUE dispatch binding is how renewal is refused at the row
-- level: one run, one capability, never a second for the same
-- dispatch. Nothing in Kanban deletes a minted capability.

CREATE TABLE capabilities (
    id INTEGER PRIMARY KEY,
    dispatch_request_id INTEGER NOT NULL UNIQUE REFERENCES dispatch_requests (id),
    ticket_id INTEGER NOT NULL REFERENCES tickets (id),
    lane_id INTEGER NOT NULL REFERENCES lanes (id),
    role TEXT NOT NULL CHECK (role IN ('implementer', 'reviewer')),
    reviewer_slot_id INTEGER,
    operations TEXT NOT NULL CHECK (length(trim(operations)) > 0),
    status TEXT NOT NULL CHECK (status IN ('active', 'settled')),
    minted_at INTEGER NOT NULL CHECK (minted_at >= 0),
    settled_at INTEGER,
    CHECK ((role = 'implementer' AND reviewer_slot_id IS NULL)
        OR (role = 'reviewer' AND reviewer_slot_id IS NOT NULL)),
    CHECK (status = 'active' OR settled_at IS NOT NULL)
);

CREATE TRIGGER capabilities_refuse_delete
BEFORE DELETE ON capabilities
BEGIN
    SELECT RAISE(ABORT, 'capabilities are durable mint records; they are never deleted');
END;
