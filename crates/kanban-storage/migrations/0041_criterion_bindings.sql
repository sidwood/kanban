CREATE TABLE criterion_bindings (
    ticket_id INTEGER NOT NULL REFERENCES tickets(id),
    criterion_index INTEGER NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('acceptance', 'task')),
    evidence_id INTEGER NOT NULL DEFAULT 0,
    tip TEXT NOT NULL,
    review TEXT NOT NULL CHECK (review IN ('pending', 'validated', 'rejected')),
    satisfied INTEGER NOT NULL CHECK (satisfied IN (0, 1)),
    void INTEGER NOT NULL CHECK (void IN (0, 1)),
    PRIMARY KEY (ticket_id, criterion_index)
);
