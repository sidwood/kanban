-- 0001 initial schema
--
-- The audit trail and activity timeline exist from the first
-- migration (ADR-0002, DR-SS-04) and accept inserts only: the
-- triggers refuse every update and delete at the schema level,
-- so no code path can work around the append-only rule.

CREATE TABLE audit_events (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    kind        TEXT NOT NULL,
    detail      TEXT NOT NULL,
    recorded_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

CREATE TRIGGER audit_events_append_only_update
BEFORE UPDATE ON audit_events
BEGIN
    SELECT RAISE(ABORT, 'audit_events is append-only');
END;

CREATE TRIGGER audit_events_append_only_delete
BEFORE DELETE ON audit_events
BEGIN
    SELECT RAISE(ABORT, 'audit_events is append-only');
END;

CREATE TABLE timeline_events (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    kind        TEXT NOT NULL,
    detail      TEXT NOT NULL,
    recorded_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

CREATE TRIGGER timeline_events_append_only_update
BEFORE UPDATE ON timeline_events
BEGIN
    SELECT RAISE(ABORT, 'timeline_events is append-only');
END;

CREATE TRIGGER timeline_events_append_only_delete
BEFORE DELETE ON timeline_events
BEGIN
    SELECT RAISE(ABORT, 'timeline_events is append-only');
END;
