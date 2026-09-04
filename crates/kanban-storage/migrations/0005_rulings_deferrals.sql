-- 0005 rulings and deferrals
--
-- Immutable operator decisions and finding deferrals (KAN-S2-US3,
-- DR-AE-03). Supersession appends a new row referencing the
-- original; no update or delete path exists.

CREATE TABLE rulings (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    project_id    TEXT NOT NULL,
    entity_kind   TEXT,
    entity_id     TEXT,
    summary       TEXT NOT NULL,
    supersedes_id INTEGER REFERENCES rulings(id),
    recorded_at   TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

CREATE INDEX idx_rulings_project_recorded
    ON rulings (project_id, recorded_at);

CREATE INDEX idx_rulings_project_entity
    ON rulings (project_id, entity_kind, entity_id);

CREATE TRIGGER rulings_append_only_update
BEFORE UPDATE ON rulings
BEGIN
    SELECT RAISE(ABORT, 'rulings is append-only');
END;

CREATE TRIGGER rulings_append_only_delete
BEFORE DELETE ON rulings
BEGIN
    SELECT RAISE(ABORT, 'rulings is append-only');
END;

CREATE TABLE deferrals (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    project_id    TEXT NOT NULL,
    finding_id    TEXT NOT NULL,
    reason        TEXT NOT NULL,
    supersedes_id INTEGER REFERENCES deferrals(id),
    recorded_at   TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

CREATE INDEX idx_deferrals_project_recorded
    ON deferrals (project_id, recorded_at);

CREATE INDEX idx_deferrals_project_finding
    ON deferrals (project_id, finding_id);

CREATE TRIGGER deferrals_append_only_update
BEFORE UPDATE ON deferrals
BEGIN
    SELECT RAISE(ABORT, 'deferrals is append-only');
END;

CREATE TRIGGER deferrals_append_only_delete
BEFORE DELETE ON deferrals
BEGIN
    SELECT RAISE(ABORT, 'deferrals is append-only');
END;
