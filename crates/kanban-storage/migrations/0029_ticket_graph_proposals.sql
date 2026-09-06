-- 0029 ticket graph proposals
--
-- One row per recorded Ticket graph proposal (DR-PS-16): the Spec and
-- the Spec content version it is proposed against, the complete
-- Ticket set and its dependency edges as JSON, and the closed
-- proposed/approved lifecycle. The partial UNIQUE keeps one approved
-- graph per Spec version — a second approval of the same version is
-- refused at the schema as much as at the gate — and approval's
-- Ticket pins live in the `tickets.pinned_version` column 0027 added.

CREATE TABLE ticket_graph_proposals (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    spec_id      INTEGER NOT NULL REFERENCES specs (id),
    spec_version INTEGER NOT NULL CHECK (spec_version > 0),
    state        TEXT NOT NULL CHECK (state IN ('proposed', 'approved')),
    tickets      TEXT NOT NULL,
    edges        TEXT NOT NULL,
    version      INTEGER NOT NULL CHECK (version > 0),
    created_at   TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

CREATE UNIQUE INDEX ticket_graph_proposals_one_approved
    ON ticket_graph_proposals (spec_id, spec_version)
    WHERE state = 'approved';
