-- 0022 ticket dependencies
--
-- Registered Ticket dependency edges and the explicit external
-- blockers that carry unregistered waiting work (DR-DE-02,
-- DR-DE-04). Edges may cross Specs and registered Projects, so the
-- rows name Tickets alone; the CHECK keeps a Ticket from depending
-- on itself, the UNIQUE keeps each edge registered once, and the
-- foreign keys keep both endpoints registered Tickets. Blockers name
-- their waiting work in prose: the CHECK refuses a blank description
-- and the UNIQUE keeps the same waiting work recorded once per
-- Ticket. Cycle rejection and readiness live in the domain; the
-- schema holds the relations the graph and the projection rehydrate.

CREATE TABLE ticket_dependencies (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    from_ticket INTEGER NOT NULL REFERENCES tickets (id),
    to_ticket   INTEGER NOT NULL REFERENCES tickets (id),
    created_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    CHECK (from_ticket <> to_ticket),
    UNIQUE (from_ticket, to_ticket)
);

CREATE TABLE ticket_blockers (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    ticket_id   INTEGER NOT NULL REFERENCES tickets (id),
    description TEXT NOT NULL CHECK (length(trim(description)) > 0),
    created_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    UNIQUE (ticket_id, description)
);
