-- 0026 execution profiles
--
-- The central catalogue of named Execution Profiles (CONTEXT.md): one
-- row per entry carrying the closed schema — harness, model, effort,
-- usage pool, and the fallback policy as the profile another entry
-- names by reference — under a name that is unique and immutable.
-- Nothing is deleted: retiring keeps the row with every recorded
-- fact, and the trigger refuses deletes at the schema level. Tickets
-- gain the assignment reference as a stored name: a name, not a
-- value, so catalogue changes never rewrite what a past assignment
-- named (DR-EP-05); the reference guards at the schema level that it
-- names a catalogue entry.

CREATE TABLE execution_profiles (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    name       TEXT NOT NULL UNIQUE,
    harness    TEXT NOT NULL CHECK (trim(harness) <> ''),
    model      TEXT NOT NULL CHECK (trim(model) <> ''),
    effort     TEXT NOT NULL CHECK (trim(effort) <> ''),
    usage_pool TEXT NOT NULL CHECK (trim(usage_pool) <> ''),
    fallback   TEXT REFERENCES execution_profiles (name),
    retired    INTEGER NOT NULL DEFAULT 0 CHECK (retired IN (0, 1)),
    version    INTEGER NOT NULL CHECK (version > 0),
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    retired_at TEXT
);

CREATE TRIGGER execution_profiles_refuse_delete
BEFORE DELETE ON execution_profiles
BEGIN
    SELECT RAISE(ABORT, 'execution profiles are retired, never deleted');
END;

ALTER TABLE tickets ADD COLUMN profile TEXT REFERENCES execution_profiles (name);
