-- 0020 tickets
--
-- A Ticket is one independently grabbable unit of work: rows in
-- `tickets` carry the Project, the minted number, the kind whose
-- schema the Ticket carries, the priority, the lifecycle state, and
-- the kind-specific fields — the Spec attachment, a Bug or Task
-- title, an Implementation slice description, and the story-linked
-- criteria stored as JSON. The schema-level CHECK keeps each kind to
-- exactly its own fields, the UNIQUE keeps minted numbers from
-- colliding, and the trigger keeps Tickets never deleted. Lifecycle
-- transitions land with KAN-T21; every row starts in draft.

CREATE TABLE tickets (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    project_id INTEGER NOT NULL REFERENCES projects (id),
    number     INTEGER NOT NULL CHECK (number > 0),
    kind       TEXT NOT NULL CHECK (kind IN ('implementation', 'bug', 'task')),
    priority   TEXT NOT NULL CHECK (priority IN ('urgent', 'high', 'normal', 'low')),
    state      TEXT NOT NULL CHECK (state IN (
        'draft', 'parked', 'blocked', 'scheduled', 'ready', 'active',
        'in_review', 'approved', 'landing', 'done', 'cancelled',
        'superseded'
    )),
    spec_id    INTEGER REFERENCES specs (id),
    title      TEXT,
    slice      TEXT,
    criteria   TEXT NOT NULL DEFAULT '[]',
    version    INTEGER NOT NULL CHECK (version > 0),
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    UNIQUE (project_id, number),
    CHECK (
        (kind = 'implementation'
            AND spec_id IS NOT NULL
            AND slice IS NOT NULL
            AND title IS NULL
            AND criteria <> '[]')
        OR (kind IN ('bug', 'task')
            AND title IS NOT NULL
            AND slice IS NULL
            AND criteria = '[]')
    )
);

CREATE TRIGGER tickets_refuse_delete
BEFORE DELETE ON tickets
BEGIN
    SELECT RAISE(ABORT, 'tickets are never deleted; supersede or cancel them');
END;
