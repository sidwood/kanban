-- 0036 ticket review configurations
--
-- One staged review configuration per Ticket (CONTEXT.md, DR-EP-09):
-- the ordered stages of parallel slots — each required or optional,
-- occupied by a human or by a catalogue profile named by reference —
-- stored as JSON in the shape the wire carries, with the aggregate
-- version optimistic checks guard. Separation is validated before
-- the write, so the row records only what the domain accepted; the
-- schema-level CHECKs mirror that gate: a non-empty stage list of
-- valid JSON, and a positive version.

CREATE TABLE ticket_review_configurations (
    ticket_id  INTEGER PRIMARY KEY REFERENCES tickets(id),
    stages     TEXT NOT NULL CHECK (json_valid(stages)
                                    AND json_array_length(stages) > 0),
    version    INTEGER NOT NULL CHECK (version > 0),
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);
