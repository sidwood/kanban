-- Walkthrough proof is a first-class criterion kind. SQLite cannot
-- ALTER a CHECK constraint, so the table is rebuilt in place.
CREATE TABLE criterion_bindings_v52 (
    ticket_id INTEGER NOT NULL REFERENCES tickets(id),
    criterion_index INTEGER NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('acceptance', 'task', 'walkthrough')),
    evidence_id INTEGER NOT NULL DEFAULT 0,
    tip TEXT NOT NULL,
    review TEXT NOT NULL CHECK (review IN ('pending', 'validated', 'rejected')),
    satisfied INTEGER NOT NULL CHECK (satisfied IN (0, 1)),
    void INTEGER NOT NULL CHECK (void IN (0, 1)),
    PRIMARY KEY (ticket_id, criterion_index)
);
INSERT INTO criterion_bindings_v52 SELECT * FROM criterion_bindings;
DROP TABLE criterion_bindings;
ALTER TABLE criterion_bindings_v52 RENAME TO criterion_bindings;
